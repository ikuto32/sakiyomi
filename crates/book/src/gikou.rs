//! Gikou (技巧) `book.bin`. Implemented from the format description; no Gikou code is used.
//!
//! ```text
//! header  20992 B: i64 psq[32 pieces][81 squares], i64 hands[2 colours][16 piece types]
//! entries 32 B each, sorted by key as signed i64:
//!         i64 key, u32 move, u32 frequency, u32 win_count, u32 opening, i32 score, 4 B padding
//! ```
//! The key of a position: if White is to move, rotate the position 180° and swap colours
//! first; then sum (wrapping) `psq[piece][square]` over all squares plus `count × hands[c][t]`.
//! The seed tables are stored in the header, so a reader uses the file's seeds and a writer
//! may choose any. Moves are stored from Black's view (flipped for White-to-move positions):
//! bits 0-6 to, 7 promote, 8-14 from, 15 drop, 16-20 moved piece, 21-25 captured piece
//! (colour = +16). Gikou picks moves with probability proportional to `win_count`.

use std::collections::{HashMap, HashSet, VecDeque};

use sakiyomi_kifu::notation::{HAND_KINDS, try_make_move};
use sakiyomi_kifu::shogi_core::rules::Board;
use sakiyomi_kifu::shogi_core::{Move16, START_SFEN};

use crate::sbk::{from_sbk_kind, to_sbk_kind};
use crate::{Book, BookError, BookMove, BookPosition, Move16Ser, ReadOptions, Report, Result, flip_board, flip_move};

pub const HEADER_LEN: usize = 32 * 81 * 8 + 2 * 16 * 8;
pub const ENTRY_LEN: usize = 32;
const SCORE_NONE: i32 = 32601;
/// Gikou only builds books from the first 50 plies; walking further is pointless by default.
const DEFAULT_MAX_PLY: u32 = 256;

struct Seeds {
    psq: Vec<i64>,
    hands: [[i64; 16]; 2],
}

impl Seeds {
    fn from_header(h: &[u8]) -> Self {
        let rd = |i: usize| i64::from_le_bytes(h[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
        let psq = (0..32 * 81).map(rd).collect();
        let mut hands = [[0i64; 16]; 2];
        for (c, row) in hands.iter_mut().enumerate() {
            for (t, v) in row.iter_mut().enumerate() {
                *v = rd(32 * 81 + c * 16 + t);
            }
        }
        Seeds { psq, hands }
    }

    /// Deterministic seeds (SplitMix64), filling exactly the slots Gikou uses.
    fn generate() -> Self {
        let mut x = 0x5AC1_7E5E_ED00_0001u64;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) as i64
        };
        let mut psq = vec![0i64; 32 * 81];
        for sq in 0..81 {
            for p in 1..32usize {
                if p != 13 && p != 16 && p != 29 {
                    psq[p * 81 + sq] = next();
                }
            }
        }
        let mut hands = [[0i64; 16]; 2];
        for row in hands.iter_mut() {
            for v in row.iter_mut().take(8).skip(1) {
                *v = next();
            }
        }
        Seeds { psq, hands }
    }

    fn header(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN);
        for v in &self.psq {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for row in &self.hands {
            for v in row {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Key of a Black-to-move position (callers flip White-to-move positions first).
    fn key_black(&self, b: &Board) -> i64 {
        let mut k = 0i64;
        // Every square contributes, empty ones through psq[0][sq] (zero in Gikou's own files).
        for (sq, &p) in b.squares.iter().enumerate() {
            let gp = if p == 0 { 0 } else { usize::from(to_sbk_kind(p & 15)) | usize::from(p >> 4) << 4 };
            k = k.wrapping_add(self.psq[gp * 81 + sq]);
        }
        for (c, hand) in b.hands.iter().enumerate() {
            for (i, &n) in hand.iter().enumerate() {
                // Gikou hand types 1..7 = P L N S G B R, the same order as shogi-core's hands.
                let t = i + 1;
                debug_assert_eq!(usize::from(to_sbk_kind(HAND_KINDS[i])), t);
                k = k.wrapping_add(self.hands[c][t].wrapping_mul(i64::from(n)));
            }
        }
        k
    }

    /// (key, position as stored, flipped?)
    fn key(&self, b: &Board) -> (i64, Board, bool) {
        if b.side == 1 {
            let f = flip_board(b);
            (self.key_black(&f), f, true)
        } else {
            (self.key_black(b), *b, false)
        }
    }
}

fn gikou_piece(p: u8) -> u32 {
    if p == 0 { 0 } else { u32::from(to_sbk_kind(p & 15)) | u32::from(p >> 4) << 4 }
}

/// Encode a move on a Black-to-move board.
fn encode_move(b: &Board, mv: Move16) -> u32 {
    let to = mv.destination().index() as u32;
    match mv.source() {
        None => to | 1 << 15 | gikou_piece(mv.drop_kind().unwrap_or(1)) << 16,
        Some(s) => {
            let from = s.index();
            to | u32::from(mv.promotes()) << 7
                | (from as u32) << 8
                | gikou_piece(b.squares[from]) << 16
                | gikou_piece(b.squares[to as usize]) << 21
        }
    }
}

/// Decode on a Black-to-move board, checking moved/captured pieces like Gikou does.
fn decode_move(b: &Board, w: u32) -> Option<Move16> {
    let to = (w & 127) as usize;
    let piece = (w >> 16 & 31) as u8;
    let kind = from_sbk_kind(piece & 15)?;
    let mv = if w >> 15 & 1 == 1 {
        try_make_move(None, to, false, kind)?
    } else {
        let from = (w >> 8 & 127) as usize;
        if from >= 81 || to >= 81 || gikou_piece(b.squares[from]) != u32::from(piece) || gikou_piece(b.squares[to]) != (w >> 21 & 31) {
            return None;
        }
        try_make_move(Some(from), to, w >> 7 & 1 == 1, 0)?
    };
    b.legal_moves().contains(&mv).then_some(mv)
}

struct Entry {
    mv: u32,
    frequency: u32,
    wins: u32,
    score: i32,
}

pub fn read(bytes: &[u8], opts: &ReadOptions) -> Result<(Book, Report)> {
    if bytes.len() < HEADER_LEN || !(bytes.len() - HEADER_LEN).is_multiple_of(ENTRY_LEN) {
        return Err(BookError::Corrupt(format!("size {} is not {} + 32·n", bytes.len(), HEADER_LEN)));
    }
    let seeds = Seeds::from_header(&bytes[..HEADER_LEN]);
    let n = (bytes.len() - HEADER_LEN) / ENTRY_LEN;
    if n > opts.limits.max_positions.saturating_mul(4) {
        return Err(BookError::TooLarge { what: "entries", limit: opts.limits.max_positions.saturating_mul(4) });
    }
    let mut by_key: HashMap<i64, Vec<Entry>> = HashMap::new();
    for e in bytes[HEADER_LEN..].as_chunks::<ENTRY_LEN>().0 {
        let u = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
        let key = i64::from_le_bytes(e[..8].try_into().unwrap_or([0; 8]));
        by_key.entry(key).or_default().push(Entry { mv: u(8), frequency: u(12), wins: u(16), score: u(24) as i32 });
    }
    // Positions are only identifiable through their key, so walk the book from known roots.
    let mut roots: Vec<Board> = vec![Board::from_sfen(START_SFEN).map_err(|e| BookError::Corrupt(e.to_string()))?];
    for r in &opts.roots {
        roots.push(sakiyomi_kifu::record::parse_sfen(r).map_err(|e| BookError::Corrupt(e.to_string()))?);
    }
    let max_ply = opts.max_ply.unwrap_or(DEFAULT_MAX_PLY);
    let mut book = Book::default();
    let mut report = Report::default();
    let mut used: HashSet<i64> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(Board, u32)> = roots.into_iter().map(|b| (b, 0)).collect();
    while let Some((board, depth)) = queue.pop_front() {
        let key = Book::key(&board);
        if !seen.insert(key.clone()) || depth > max_ply {
            continue;
        }
        if book.positions.len() >= opts.limits.max_positions {
            return Err(BookError::TooLarge { what: "positions", limit: opts.limits.max_positions });
        }
        let (k, stored, flipped) = seeds.key(&board);
        let Some(entries) = by_key.get(&k) else { continue };
        used.insert(k);
        let mut pos = BookPosition { ply: Some(board.move_number), ..Default::default() };
        for e in entries {
            let Some(m) = decode_move(&stored, e.mv) else {
                report.illegal_moves += 1;
                continue;
            };
            let real = if flipped { flip_move(m) } else { Some(m) };
            let Some(real) = real.filter(|r| board.legal_moves().contains(r)) else {
                report.illegal_moves += 1;
                continue;
            };
            if pos.find_mut(real).is_some() {
                continue;
            }
            // Gikou stores scores from the real Black's view; the model uses side to move.
            let value = (e.score != SCORE_NONE).then(|| if board.side == 0 { e.score } else { -e.score });
            pos.moves.push(BookMove {
                mv: Move16Ser(real),
                ponder: None,
                value,
                depth: None,
                count: u64::from(e.frequency.max(e.wins)),
                wins: Some(u64::from(e.wins)),
                mark: Default::default(),
                comment: String::new(),
            });
            queue.push_back((board.after_generated(real), depth + 1));
        }
        if !pos.moves.is_empty() {
            book.positions.insert(key, pos);
        }
    }
    report.unreachable = by_key.keys().filter(|k| !used.contains(k)).count();
    Ok((book, report))
}

pub fn write(book: &Book) -> Vec<u8> {
    let seeds = Seeds::generate();
    let mut entries: Vec<(i64, u32, u32, u32, i32)> = Vec::new();
    let mut seen: HashSet<(i64, u32)> = HashSet::new();
    for (k, pos) in &book.positions {
        let Some(board) = Book::board(k) else { continue };
        let (key, stored, flipped) = seeds.key(&board);
        for m in &pos.moves {
            let mv = if flipped { flip_move(m.mv.0) } else { Some(m.mv.0) };
            let Some(mv) = mv else { continue };
            let code = encode_move(&stored, mv);
            if !seen.insert((key, code)) {
                continue;
            }
            // Selection weight lives in win_count; keep it ≥ 1 so every book move is playable.
            let wins = m.wins.unwrap_or(m.count).clamp(1, u64::from(u32::MAX)) as u32;
            let freq = m.count.clamp(u64::from(wins), u64::from(u32::MAX)) as u32;
            let score = m.value.map(|v| if board.side == 0 { v } else { -v }).unwrap_or(SCORE_NONE);
            entries.push((key, code, freq, wins, score));
        }
    }
    entries.sort_by_key(|e| e.0);
    let mut out = seeds.header();
    out.reserve(entries.len() * ENTRY_LEN);
    for (key, mv, freq, wins, score) in entries {
        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&mv.to_le_bytes());
        out.extend_from_slice(&freq.to_le_bytes());
        out.extend_from_slice(&wins.to_le_bytes());
        out.extend_from_slice(&0x1FFu32.to_le_bytes()); // all opening classes
        out.extend_from_slice(&score.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BookFormat;

    fn mv(s: &str) -> Move16 {
        Move16::from_usi(s).unwrap()
    }

    #[test]
    fn move_encoding_matches_format_description() {
        let start = Board::from_sfen(START_SFEN).unwrap();
        assert_eq!(encode_move(&start, mv("7g7f")), 0x0001_3C3B);
        let b = Board::from_sfen("lnsgkgsnl/1r5b1/pppppp1pp/6p2/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL b - 3").unwrap();
        assert_eq!(encode_move(&b, mv("8h2b+")), 0x02C6_468A);
        let d = Board::from_sfen("4k4/9/9/9/9/9/9/9/4K4 b P 1").unwrap();
        assert_eq!(encode_move(&d, mv("P*5e")), 0x0001_8028);
        assert_eq!(decode_move(&b, 0x02C6_468A), Some(mv("8h2b+")));
        // Wrong captured piece → rejected (Gikou validates this too).
        assert_eq!(decode_move(&b, 0x02C6_468A & !(31 << 21)), None);
        assert_eq!(HEADER_LEN, 20992);
    }

    #[test]
    fn white_positions_share_keys_with_their_mirror() {
        let s = Seeds::generate();
        let start = Board::from_sfen(START_SFEN).unwrap();
        let after = start.after_generated(mv("7g7f"));
        let mirror = flip_board(&after);
        assert_eq!(s.key(&after).0, s.key(&mirror).0);
        assert_ne!(s.key(&start).0, s.key(&after).0);
    }

    #[test]
    fn roundtrip_through_gikou() {
        let book = crate::tests::sample();
        let (bytes, _) = crate::write(BookFormat::Gikou, &book, &Default::default()).unwrap();
        assert_eq!((bytes.len() - HEADER_LEN) % ENTRY_LEN, 0);
        // Keys are sorted as signed integers.
        let keys: Vec<i64> = bytes[HEADER_LEN..].as_chunks::<32>().0.iter().map(|e| i64::from_le_bytes(e[..8].try_into().unwrap())).collect();
        assert!(keys.windows(2).all(|w| w[0] <= w[1]));
        let (back, report) = crate::read(BookFormat::Gikou, &bytes, &ReadOptions::default()).unwrap();
        assert_eq!((report.illegal_moves, report.unreachable), (0, 0));
        assert_eq!(back.positions.len(), book.positions.len());
        for (k, p) in &book.positions {
            let q = &back.positions[k];
            let mut a: Vec<_> = p.moves.iter().map(|m| (m.mv.0.to_usi(), m.value)).collect();
            let mut b: Vec<_> = q.moves.iter().map(|m| (m.mv.0.to_usi(), m.value)).collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{k}");
        }
    }

    #[test]
    fn hostile_gikou() {
        assert!(read(&[0u8; 100], &ReadOptions::default()).is_err());
        let mut d = vec![0u8; HEADER_LEN + 32 * 50];
        let mut x = 0x9E37u64;
        for b in d.iter_mut() {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            *b = (x >> 40) as u8;
        }
        let r = read(&d, &ReadOptions::default());
        assert!(r.is_ok());
    }
}
