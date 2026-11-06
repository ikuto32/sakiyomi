//! ShogiGUI book (`.sbk`): one protobuf `SBook` message.
//!
//! ```text
//! SBook      { 1 Author: string, 2 Description: string, 3 BookStates: repeated SBookState }
//! SBookState { 1 Id, 2 BoardKey: u64, 3 HandKey: u32, 4 Games, 5 WonBlack, 6 WonWhite,
//!              7 Position: SFEN (roots only), 8 Comment, 9 Moves: repeated SBookMove,
//!              10 Evals: repeated SBookEval }
//! SBookMove  { 1 Move: int32 word, 2 Evaluation: enum, 3 Weight, 4 NextStateId (-1 none) }
//! SBookEval  { 1 Value, 2 Depth, 3 SelDepth, 4 Nodes: int64, 5 Variation: string, 6 EngineName }
//! ```
//! Move word: bits 0-3 from rank, 4-7 from file (0 = drop), 8-11 to rank, 12-15 to file,
//! 19 promote, 20-23 captured piece (colourless), 24-28 moving piece (coloured), 31 White.
//! Pieces: 1 P 2 L 3 N 4 S 5 G 6 B 7 R 8 K, +8 promoted, +16 White.
//!
//! Only root states carry a position; the others are reached by replaying moves. The
//! BoardKey/HandKey hashes are ShogiGUI-internal; like ShogiGUI's own converter we write 0.

use std::collections::{HashMap, VecDeque};

use sakiyomi_kifu::notation::{self, file_of, rank_of, square, try_make_move};
use sakiyomi_kifu::shogi_core::rules::Board;
use sakiyomi_kifu::shogi_core::{Move16, START_SFEN};

use crate::protobuf::{LEN, Reader, VARINT, Writer};
use crate::{Book, BookError, BookLimits, BookMove, BookPosition, Mark, Move16Ser, PositionEval, Report, Result, WriteOptions};

/// shogi-core kind (1 P 2 L 3 N 4 S 5 B 6 R 7 G 8 K, +8) → ShogiGUI/Gikou piece type.
pub(crate) fn to_sbk_kind(k: u8) -> u8 {
    match k {
        5 => 6,
        6 => 7,
        7 => 5,
        13 => 14,
        14 => 15,
        other => other,
    }
}

pub(crate) fn from_sbk_kind(k: u8) -> Option<u8> {
    Some(match k {
        1..=4 | 8..=12 => k,
        5 => 7,
        6 => 5,
        7 => 6,
        14 => 13,
        15 => 14,
        _ => return None,
    })
}

fn encode_move(board: &Board, mv: Move16) -> i32 {
    let to = mv.destination().index();
    let mut w: u32 = (rank_of(to) as u32) << 8 | (file_of(to) as u32) << 12;
    let side = u32::from(board.side);
    match mv.source() {
        Some(s) => {
            let from = s.index();
            w |= rank_of(from) as u32 | (file_of(from) as u32) << 4;
            if mv.promotes() {
                w |= 1 << 19;
            }
            let captured = board.squares[to] & 15;
            if captured != 0 {
                w |= u32::from(to_sbk_kind(captured)) << 20;
            }
            w |= (u32::from(to_sbk_kind(board.squares[from] & 15)) | side << 4) << 24;
        }
        None => w |= (u32::from(to_sbk_kind(mv.drop_kind().unwrap_or(1))) | side << 4) << 24,
    }
    (w | side << 31) as i32
}

/// Decode a move word in the context of `board`, checking it against the board contents.
fn decode_move(board: &Board, word: i32) -> Option<Move16> {
    let w = word as u32;
    let (fr, ff, tr, tf) = ((w & 15) as usize, (w >> 4 & 15) as usize, (w >> 8 & 15) as usize, (w >> 12 & 15) as usize);
    if !(1..=9).contains(&tr) || !(1..=9).contains(&tf) {
        return None;
    }
    let to = square(tf, tr);
    let piece = (w >> 24 & 0x1f) as u8;
    let kind = from_sbk_kind(piece & 15)?;
    let mv = if fr == 0 && ff == 0 {
        try_make_move(None, to, false, kind)?
    } else {
        if !(1..=9).contains(&fr) || !(1..=9).contains(&ff) {
            return None;
        }
        let from = square(ff, fr);
        if board.squares[from] & 15 != kind {
            return None;
        }
        try_make_move(Some(from), to, w >> 19 & 1 == 1, 0)?
    };
    board.legal_moves().contains(&mv).then_some(mv)
}

struct RawMove {
    word: i32,
    mark: u64,
    weight: i32,
    next: i32,
}

#[derive(Default)]
struct RawState {
    games: i32,
    won: [i32; 2],
    position: Option<String>,
    comment: String,
    moves: Vec<RawMove>,
    evals: Vec<PositionEval>,
}

fn read_move(b: &[u8]) -> Result<RawMove> {
    let mut r = Reader::new(b);
    let mut m = RawMove { word: 0, mark: 0, weight: 0, next: -1 };
    while !r.done() {
        let (f, wire) = r.key()?;
        match (f, wire) {
            (1, VARINT) => m.word = r.int32()?,
            (2, VARINT) => m.mark = r.varint()?,
            (3, VARINT) => m.weight = r.int32()?,
            (4, VARINT) => m.next = r.int32()?,
            _ => r.skip(wire)?,
        }
    }
    Ok(m)
}

fn read_eval(b: &[u8]) -> Result<PositionEval> {
    let mut r = Reader::new(b);
    let mut e = PositionEval::default();
    while !r.done() {
        let (f, wire) = r.key()?;
        match (f, wire) {
            (1, VARINT) => e.value = r.int32()?,
            (2, VARINT) => e.depth = r.int32()?.max(0) as u32,
            (3, VARINT) => e.seldepth = r.int32()?.max(0) as u32,
            (4, VARINT) => e.nodes = r.varint()?,
            (5, LEN) => e.variation = r.string()?,
            (6, LEN) => e.engine = Some(r.string()?),
            _ => r.skip(wire)?,
        }
    }
    Ok(e)
}

fn read_state(b: &[u8], limits: &BookLimits) -> Result<RawState> {
    let mut r = Reader::new(b);
    let mut s = RawState::default();
    while !r.done() {
        let (f, wire) = r.key()?;
        match (f, wire) {
            (4, VARINT) => s.games = r.int32()?,
            (5, VARINT) => s.won[0] = r.int32()?,
            (6, VARINT) => s.won[1] = r.int32()?,
            (7, LEN) => s.position = Some(r.string()?),
            (8, LEN) => s.comment = r.string()?,
            (9, LEN) => {
                if s.moves.len() >= limits.max_moves_per_position {
                    return Err(BookError::TooLarge { what: "moves in one position", limit: limits.max_moves_per_position });
                }
                s.moves.push(read_move(r.bytes()?)?);
            }
            (10, LEN) => s.evals.push(read_eval(r.bytes()?)?),
            (1..=10, _) => {
                // Known field with the wrong wire type is corrupt; otherwise skip its value.
                r.expect(wire, if f == 7 || f == 8 || f >= 9 { LEN } else { VARINT })?;
                r.skip(wire)?;
            }
            _ => r.skip(wire)?,
        }
    }
    Ok(s)
}

pub fn read(bytes: &[u8], limits: &BookLimits) -> Result<(Book, Report)> {
    let mut r = Reader::new(bytes);
    let mut book = Book::default();
    let mut states: Vec<RawState> = Vec::new();
    while !r.done() {
        let (f, wire) = r.key()?;
        match (f, wire) {
            (1, LEN) => book.author = r.string()?,
            (2, LEN) => book.description = r.string()?,
            (3, LEN) => {
                if states.len() >= limits.max_positions {
                    return Err(BookError::TooLarge { what: "positions", limit: limits.max_positions });
                }
                states.push(read_state(r.bytes()?, limits)?);
            }
            _ => r.skip(wire)?,
        }
    }
    let mut report = Report::default();
    // Assign boards: roots carry a Position; state 0 without one is the initial position.
    let mut boards: Vec<Option<Board>> = vec![None; states.len()];
    let mut queue = VecDeque::new();
    for (i, s) in states.iter().enumerate() {
        let sfen = match &s.position {
            Some(p) if !p.trim().is_empty() => Some(p.trim().to_owned()),
            _ if i == 0 => Some(START_SFEN.to_owned()),
            _ => None,
        };
        if let Some(sfen) = sfen {
            match sakiyomi_kifu::record::parse_sfen(&sfen) {
                Ok(b) => {
                    boards[i] = Some(b);
                    queue.push_back(i);
                }
                Err(_) => report.invalid_positions += 1,
            }
        }
    }
    let mut decoded: Vec<Vec<(Move16, &RawMove)>> = (0..states.len()).map(|_| Vec::new()).collect();
    while let Some(i) = queue.pop_front() {
        let Some(board) = boards[i] else { continue };
        for m in &states[i].moves {
            let Some(mv) = decode_move(&board, m.word) else {
                report.illegal_moves += 1;
                continue;
            };
            decoded[i].push((mv, m));
            let child = board.after_generated(mv);
            if let Some(next) = usize::try_from(m.next).ok().filter(|&n| n < states.len()) {
                match boards[next] {
                    None => {
                        boards[next] = Some(child);
                        queue.push_back(next);
                    }
                    Some(existing) if existing.key() != child.key() => report.invalid_positions += 1,
                    Some(_) => {}
                }
            }
        }
    }
    for (i, s) in states.iter().enumerate() {
        let Some(board) = boards[i] else {
            report.unreachable += 1;
            continue;
        };
        let key = Book::key(&board);
        let pos = book.positions.entry(key).or_insert_with(BookPosition::default);
        pos.ply = Some(pos.ply.map_or(board.move_number, |p| p.min(board.move_number)));
        for &(mv, m) in &decoded[i] {
            if pos.find_mut(mv).is_some() {
                continue;
            }
            pos.moves.push(BookMove {
                mv: Move16Ser(mv),
                ponder: None,
                value: None,
                depth: None,
                count: m.weight.max(0) as u64,
                wins: None,
                mark: match m.mark {
                    1 => Mark::Forced,
                    2 => Mark::Good,
                    3 => Mark::Bad,
                    4 => Mark::Blunder,
                    _ => Mark::None,
                },
                comment: String::new(),
            });
        }
        if pos.comment.is_empty() {
            pos.comment = s.comment.clone();
        }
        pos.games = Some(s.games.max(0) as u32);
        pos.wins = Some([s.won[0].max(0) as u32, s.won[1].max(0) as u32]);
        pos.evals.extend(s.evals.iter().cloned());
    }
    Ok((book, report))
}

fn write_state(w: &mut Writer, id: usize, board: &Board, pos: Option<&BookPosition>, root: bool, ids: &HashMap<String, usize>, opts: &WriteOptions) {
    let mut s = Writer::default();
    s.int32(1, id as i32);
    s.uint64(2, 0);
    s.uint64(3, 0);
    let games = pos.and_then(|p| p.games).unwrap_or(1).min(i32::MAX as u32) as i32;
    let won = pos.and_then(|p| p.wins).unwrap_or([0, 0]);
    s.int32(4, games);
    s.int32(5, won[0].min(i32::MAX as u32) as i32);
    s.int32(6, won[1].min(i32::MAX as u32) as i32);
    if root {
        let mut b = *board;
        b.move_number = b.move_number.max(1);
        s.string(7, &b.to_sfen());
    }
    if let Some(p) = pos {
        if !p.comment.is_empty() {
            s.string(8, &p.comment);
        }
        for m in &p.moves {
            let mut mw = Writer::default();
            mw.int32(1, encode_move(board, m.mv.0));
            mw.int32(
                2,
                match m.mark {
                    Mark::None => 0,
                    Mark::Forced => 1,
                    Mark::Good => 2,
                    Mark::Bad => 3,
                    Mark::Blunder => 4,
                },
            );
            mw.int32(3, m.count.min(i32::MAX as u64) as i32);
            let child = Book::key(&board.after_generated(m.mv.0));
            mw.int32(4, ids.get(&child).map(|&c| c as i32).unwrap_or(-1));
            s.bytes(9, &mw.buf);
        }
        let mut evals = p.evals.clone();
        if opts.sbk_evals_from_values && evals.is_empty()
            && let Some(best) = p.best().filter(|m| m.value.is_some()) {
                let mut pv = vec![best.mv.0.to_usi()];
                if let Some(pd) = best.ponder {
                    pv.push(pd.0.to_usi());
                }
                let jp = notation::pv_to_jp(board, &pv, None);
                evals.push(PositionEval {
                    value: best.value.unwrap_or(0),
                    depth: best.depth.unwrap_or(0),
                    seldepth: best.depth.unwrap_or(0),
                    nodes: 0,
                    variation: jp,
                    engine: None,
                });
            }
        for e in &evals {
            let mut ew = Writer::default();
            ew.int32(1, e.value);
            ew.int32(2, e.depth.min(i32::MAX as u32) as i32);
            ew.int32(3, e.seldepth.min(i32::MAX as u32) as i32);
            ew.uint64(4, e.nodes);
            ew.string(5, &e.variation);
            if let Some(n) = &e.engine {
                ew.string(6, n);
            }
            s.bytes(10, &ew.buf);
        }
    }
    w.bytes(3, &s.buf);
}

pub fn write(book: &Book, opts: &WriteOptions) -> Vec<u8> {
    // States: every book position, plus leaf states for children outside the book
    // (ShogiGUI creates a state for every move's destination).
    let mut order: Vec<(String, Board)> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let start = Board::from_sfen(START_SFEN).ok();
    // The initial position first, so ShogiGUI's "state 0 = start" convention holds.
    if let Some(s) = start {
        let k = Book::key(&s);
        if book.positions.contains_key(&k) {
            ids.insert(k.clone(), 0);
            order.push((k, s));
        }
    }
    for k in book.positions.keys() {
        if ids.contains_key(k) {
            continue;
        }
        if let Some(b) = Book::board(k) {
            ids.insert(k.clone(), order.len());
            order.push((k.clone(), b));
        }
    }
    let mut reached = vec![false; order.len()];
    let mut leaves: Vec<(String, Board)> = Vec::new();
    for (k, b) in &order {
        if let Some(p) = book.positions.get(k) {
            for m in &p.moves {
                let child = b.after_generated(m.mv.0);
                let ck = Book::key(&child);
                match ids.get(&ck) {
                    Some(&c) if c < reached.len() => reached[c] = true,
                    Some(_) => {}
                    None => {
                        ids.insert(ck.clone(), order.len() + leaves.len());
                        leaves.push((ck, child));
                    }
                }
            }
        }
    }
    // Roots carry their SFEN: positions no book move leads to, a non-initial state 0, and one
    // position of every cycle that is not reachable from those (e.g. repetition loops).
    let start_key = start.map(|s| Book::key(&s)).unwrap_or_default();
    let mut root: Vec<bool> = (0..order.len()).map(|i| !reached[i] || (i == 0 && order[i].0 != start_key)).collect();
    let mut visited = vec![false; order.len()];
    let visit = |from: usize, visited: &mut [bool]| {
        let mut stack = vec![from];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut visited[i], true) {
                continue;
            }
            let (k, b) = &order[i];
            for m in book.positions.get(k).map(|p| p.moves.as_slice()).unwrap_or_default() {
                if let Some(&c) = ids.get(&Book::key(&b.after_generated(m.mv.0)))
                    && c < order.len()
                {
                    stack.push(c);
                }
            }
        }
    };
    for i in 0..order.len() {
        if root[i] || (i == 0 && order[0].0 == start_key) {
            visit(i, &mut visited);
        }
    }
    for i in 0..order.len() {
        if !visited[i] {
            root[i] = true;
            visit(i, &mut visited);
        }
    }
    let mut w = Writer::default();
    w.string(1, &book.author);
    w.string(2, &book.description);
    for (i, (k, b)) in order.iter().enumerate() {
        write_state(&mut w, i, b, book.positions.get(k), root[i], &ids, opts);
    }
    for (j, (_, b)) in leaves.iter().enumerate() {
        write_state(&mut w, order.len() + j, b, None, false, &ids, opts);
    }
    w.buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BookFormat;

    fn mv(s: &str) -> Move16 {
        Move16::from_usi(s).unwrap()
    }

    #[test]
    fn move_words_match_shogigui() {
        // Values taken from ShogiGUI-written files.
        let start = Board::from_sfen(START_SFEN).unwrap();
        assert_eq!(encode_move(&start, mv("7g7f")) as u32, 0x0100_7677);
        let b = start.after_generated(mv("7g7f"));
        assert_eq!(encode_move(&b, mv("3c3d")) as u32, 0x9100_3433);
        let b2 = b.after_generated(mv("3c3d"));
        let b3 = b2.after_generated(mv("2g2f"));
        // 2b8h+ by White capturing a bishop.
        assert_eq!(encode_move(&b3, mv("2b8h+")) as u32, 0x9668_8822);
        for (board, m) in [(&start, "7g7f"), (&b, "3c3d"), (&b3, "2b8h+")] {
            assert_eq!(decode_move(board, encode_move(board, mv(m))), Some(mv(m)));
        }
        // A word whose moving piece disagrees with the board is rejected.
        assert_eq!(decode_move(&start, 0x0200_7677), None);
    }

    #[test]
    fn roundtrip_through_sbk() {
        let book = crate::tests::sample();
        let (bytes, report) = crate::write(BookFormat::Sbk, &book, &WriteOptions::default()).unwrap();
        assert!(report.lost_fields.contains(&"move values".to_owned()));
        let (back, r) = crate::read(BookFormat::Sbk, &bytes, &crate::ReadOptions::default()).unwrap();
        assert_eq!((r.illegal_moves, r.unreachable), (0, 0));
        // The three book positions plus leaf states for unexplored children.
        let with_moves: Vec<_> = back.positions.iter().filter(|(_, p)| !p.moves.is_empty()).collect();
        assert_eq!(with_moves.len(), 3);
        for (k, p) in &book.positions {
            let q = &back.positions[k];
            assert_eq!(p.moves.iter().map(|m| (m.mv, m.count)).collect::<Vec<_>>(), q.moves.iter().map(|m| (m.mv, m.count)).collect::<Vec<_>>());
        }
        assert_eq!(back.author, "test");
        // With evals: best values survive as SBookEval entries.
        let (bytes, _) = crate::write(BookFormat::Sbk, &book, &WriteOptions { sbk_evals_from_values: true }).unwrap();
        let (back, _) = crate::read(BookFormat::Sbk, &bytes, &crate::ReadOptions::default()).unwrap();
        let start = Book::key(&Board::from_sfen(START_SFEN).unwrap());
        assert_eq!(back.positions[&start].evals[0].value, 40);
        assert!(back.positions[&start].evals[0].variation.starts_with("▲７六歩"));
    }

    #[test]
    fn cycles_without_a_root_survive() {
        // A king shuffle after 7g7f: four positions forming a loop that no other position leads to.
        let mut book = Book::default();
        let mut b = Board::from_sfen(START_SFEN).unwrap().after_generated(mv("7g7f"));
        for m in ["5a5b", "5i5h", "5b5a", "5h5i"] {
            book.add(&b, crate::BookMove { mv: crate::Move16Ser(mv(m)), ponder: None, value: Some(10), depth: None, count: 1, wins: None, mark: Default::default(), comment: String::new() });
            b = b.after_generated(mv(m));
        }
        assert_eq!(book.positions.len(), 4);
        let (bytes, _) = crate::write(BookFormat::Sbk, &book, &WriteOptions::default()).unwrap();
        let (back, r) = crate::read(BookFormat::Sbk, &bytes, &crate::ReadOptions::default()).unwrap();
        assert_eq!((r.illegal_moves, r.unreachable), (0, 0));
        assert_eq!(back.positions.values().filter(|p| !p.moves.is_empty()).count(), 4);
    }

    #[test]
    fn hostile_sbk() {
        let mut x = 0x853c49e6748fea9bu64;
        for len in [0, 1, 2, 5, 17, 100, 1000] {
            let mut d = vec![0u8; len];
            for b in d.iter_mut() {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *b = (x >> 33) as u8;
            }
            let _ = read(&d, &BookLimits::default());
        }
        // A cycle of NextStateIds and a state pointing at itself.
        let mut w = Writer::default();
        let mut s = Writer::default();
        let mut m = Writer::default();
        m.int32(1, 0x0100_7677);
        m.int32(3, 1);
        m.int32(4, 0);
        s.bytes(9, &m.buf);
        w.bytes(3, &s.buf);
        let (book, report) = read(&w.buf, &BookLimits::default()).unwrap();
        assert_eq!(book.positions.len(), 1);
        assert_eq!(report.invalid_positions, 1);
    }
}
