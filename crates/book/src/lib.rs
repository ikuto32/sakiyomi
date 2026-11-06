//! Opening books: a common model plus readers/writers for YaneuraOu `.db`, ShogiGUI `.sbk`
//! and Gikou `book.bin`, and operations (merge, filter, statistics, minimax back-up, mining).
//!
//! Positions are keyed by SFEN *without* the move number, so transpositions share one entry.
//! Every move is checked for legality on load; illegal moves are dropped and counted in the
//! [`Report`], never trusted.

pub mod db;
pub mod gikou;
pub mod miner;
pub mod peta;
mod protobuf;
pub mod sbk;

use std::collections::BTreeMap;
use std::fmt;

use sakiyomi_kifu::record::{parse_sfen, sfen_key};
use sakiyomi_kifu::shogi_core::Move16;
use sakiyomi_kifu::shogi_core::rules::Board;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mark {
    #[default]
    None,
    /// 絶対手
    Forced,
    /// 好手
    Good,
    /// 疑問手
    Bad,
    /// 悪手
    Blunder,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BookMove {
    pub mv: Move16Ser,
    pub ponder: Option<Move16Ser>,
    /// Evaluation from the side to move's point of view (YaneuraOu convention).
    pub value: Option<i32>,
    pub depth: Option<u32>,
    /// How often the move was played / its selection weight.
    pub count: u64,
    /// Games won by the side making this move (Gikou).
    pub wins: Option<u64>,
    pub mark: Mark,
    /// Comment of the move (YaneuraOu `.db`: text after `//` on the move line).
    pub comment: String,
}

impl BookMove {
    pub fn new(mv: Move16) -> Self {
        BookMove { mv: Move16Ser(mv), ponder: None, value: None, depth: None, count: 1, wins: None, mark: Mark::None, comment: String::new() }
    }
}

/// `Move16` that serialises as USI text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move16Ser(pub Move16);

impl Serialize for Move16Ser {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_usi())
    }
}

/// A stored engine analysis of a whole position (ShogiGUI `SBookEval`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PositionEval {
    pub value: i32,
    pub depth: u32,
    pub seldepth: u32,
    pub nodes: u64,
    /// Principal variation as KIF-style text (as ShogiGUI stores it).
    pub variation: String,
    pub engine: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BookPosition {
    /// Move number the position was recorded at (YaneuraOu keeps the smallest seen).
    pub ply: Option<u32>,
    pub moves: Vec<BookMove>,
    pub comment: String,
    pub games: Option<u32>,
    /// Wins for Black and White (ShogiGUI statistics).
    pub wins: Option<[u32; 2]>,
    pub evals: Vec<PositionEval>,
    /// YaneuraOu `.db`: the original text of every `sfen` section of this position, when the
    /// file lists it more than once (at different plies). The sections are merged into this
    /// entry; the text keeps what the merge loses (SPG `src.yane.byply`, spec §10.3).
    pub by_ply: String,
}

impl BookPosition {
    pub fn find_mut(&mut self, mv: Move16) -> Option<&mut BookMove> {
        self.moves.iter_mut().find(|m| m.mv.0 == mv)
    }

    /// Best move by value (then count).
    pub fn best(&self) -> Option<&BookMove> {
        self.moves.iter().max_by_key(|m| (m.value.unwrap_or(i32::MIN), m.count))
    }

    /// Sort moves like YaneuraOu: by count, then value, both descending.
    pub fn sort_moves(&mut self) {
        self.moves.sort_by(|a, b| b.count.cmp(&a.count).then(b.value.unwrap_or(i32::MIN).cmp(&a.value.unwrap_or(i32::MIN))));
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Book {
    pub author: String,
    pub description: String,
    /// SFEN without move number → position.
    pub positions: BTreeMap<String, BookPosition>,
}

/// Problems found while reading or converting; nothing here is fatal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub positions: usize,
    pub moves: usize,
    pub illegal_moves: usize,
    pub invalid_positions: usize,
    pub duplicate_positions: usize,
    /// Positions present in the file but not reachable from any root (sbk, Gikou).
    pub unreachable: usize,
    /// Written positions that a reader cannot find again without extra roots: a Gikou book
    /// stores only hash keys, so readers walk it from known positions.
    pub needs_roots: usize,
    /// Fields the target format cannot store (e.g. "value" for .sbk).
    pub lost_fields: Vec<String>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} positions, {} moves", self.positions, self.moves)?;
        for (n, what) in [
            (self.illegal_moves, "illegal moves dropped"),
            (self.invalid_positions, "invalid positions dropped"),
            (self.duplicate_positions, "duplicate positions merged"),
            (self.unreachable, "unreachable entries"),
            (self.needs_roots, "positions not reachable from the initial position (read them back with extra roots)"),
        ] {
            if n > 0 {
                write!(f, ", {n} {what}")?;
            }
        }
        if !self.lost_fields.is_empty() {
            write!(f, "; not stored by the target format: {}", self.lost_fields.join(", "))?;
        }
        Ok(())
    }
}

/// Errors that stop reading a book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookError {
    Corrupt(String),
    TooLarge { what: &'static str, limit: usize },
    Io(String),
    Unsupported(String),
}

impl fmt::Display for BookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BookError::Corrupt(m) => write!(f, "corrupt book: {m}"),
            BookError::TooLarge { what, limit } => write!(f, "{what} exceeds the limit of {limit}"),
            BookError::Io(m) => write!(f, "I/O error: {m}"),
            BookError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for BookError {}

pub type Result<T> = std::result::Result<T, BookError>;

/// Resource limits for reading books.
#[derive(Clone, Copy, Debug)]
pub struct BookLimits {
    pub max_positions: usize,
    pub max_moves_per_position: usize,
    pub max_line_bytes: usize,
}

impl Default for BookLimits {
    fn default() -> Self {
        BookLimits { max_positions: 50_000_000, max_moves_per_position: 600, max_line_bytes: 64 << 10 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BookFormat {
    /// YaneuraOu `#YANEURAOU-DB2016 1.00` text.
    YaneuraOuDb,
    /// ShogiGUI protobuf.
    Sbk,
    /// Gikou `book.bin`.
    Gikou,
}

impl BookFormat {
    pub const ALL: [BookFormat; 3] = [BookFormat::YaneuraOuDb, BookFormat::Sbk, BookFormat::Gikou];

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().trim_start_matches('.') {
            "db" | "yaneuraou" => Some(BookFormat::YaneuraOuDb),
            "sbk" | "shogigui" => Some(BookFormat::Sbk),
            "bin" | "gikou" => Some(BookFormat::Gikou),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BookFormat::YaneuraOuDb => "db",
            BookFormat::Sbk => "sbk",
            BookFormat::Gikou => "gikou",
        }
    }

    /// Guess from file contents.
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"#YANEURAOU-DB2016") || bytes.starts_with(b"sfen ") {
            Some(BookFormat::YaneuraOuDb)
        } else if bytes.len() >= gikou::HEADER_LEN && (bytes.len() - gikou::HEADER_LEN).is_multiple_of(gikou::ENTRY_LEN) {
            Some(BookFormat::Gikou)
        } else if bytes.first() == Some(&0x0a) {
            Some(BookFormat::Sbk)
        } else {
            None
        }
    }
}

/// Options for reading formats that can only be walked from known positions.
#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    pub limits: BookLimits,
    /// Extra start positions (SFEN) for Gikou books, besides the initial position.
    pub roots: Vec<String>,
    /// Maximum ply explored when walking a Gikou book.
    pub max_ply: Option<u32>,
}

pub fn read(format: BookFormat, bytes: &[u8], opts: &ReadOptions) -> Result<(Book, Report)> {
    let (mut book, mut report) = match format {
        BookFormat::YaneuraOuDb => db::read(bytes, &opts.limits)?,
        BookFormat::Sbk => sbk::read(bytes, &opts.limits)?,
        BookFormat::Gikou => gikou::read(bytes, opts)?,
    };
    report.illegal_moves += book.validate();
    report.positions = book.positions.len();
    report.moves = book.positions.values().map(|p| p.moves.len()).sum();
    Ok((book, report))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WriteOptions {
    /// .sbk: also store each position's best move value as an `SBookEval`.
    pub sbk_evals_from_values: bool,
}

pub fn write(format: BookFormat, book: &Book, opts: &WriteOptions) -> Result<(Vec<u8>, Report)> {
    let mut report = Report {
        positions: book.positions.len(),
        moves: book.positions.values().map(|p| p.moves.len()).sum(),
        ..Default::default()
    };
    let has = |f: &dyn Fn(&BookMove) -> bool| book.positions.values().any(|p| p.moves.iter().any(f));
    let bytes = match format {
        BookFormat::YaneuraOuDb => {
            if book.positions.values().any(|p| !p.comment.is_empty() || !p.evals.is_empty() || p.games.is_some()) {
                report.lost_fields.push("position comments/statistics".into());
            }
            db::write(book)
        }
        BookFormat::Sbk => {
            if has(&|m| m.value.is_some()) && !opts.sbk_evals_from_values {
                report.lost_fields.push("move values".into());
            }
            if has(&|m| m.depth.is_some()) {
                report.lost_fields.push("move depths".into());
            }
            if has(&|m| m.ponder.is_some()) {
                report.lost_fields.push("ponder moves".into());
            }
            sbk::write(book, opts)
        }
        BookFormat::Gikou => {
            if has(&|m| m.depth.is_some()) {
                report.lost_fields.push("move depths".into());
            }
            if has(&|m| m.ponder.is_some()) {
                report.lost_fields.push("ponder moves".into());
            }
            if book.positions.values().any(|p| !p.comment.is_empty()) {
                report.lost_fields.push("comments".into());
            }
            let start = Board::from_sfen(sakiyomi_kifu::shogi_core::START_SFEN).map_err(|e| BookError::Corrupt(e.to_string()))?;
            report.needs_roots = book.positions.len().saturating_sub(reachable(book, &[start]).len());
            gikou::write(book)
        }
    };
    Ok((bytes, report))
}

impl Book {
    pub fn board(key: &str) -> Option<Board> {
        parse_sfen(&format!("{key} 1")).ok()
    }

    pub fn key(board: &Board) -> String {
        sfen_key(board)
    }

    /// Drop illegal moves and invalid positions; returns how many moves were dropped.
    pub fn validate(&mut self) -> usize {
        let mut dropped = 0;
        self.positions.retain(|key, pos| {
            let Some(board) = Book::board(key) else {
                dropped += pos.moves.len();
                return false;
            };
            let legal = board.legal_moves();
            let before = pos.moves.len();
            pos.moves.retain(|m| legal.contains(&m.mv.0));
            let mut seen = std::collections::HashSet::new();
            pos.moves.retain(|m| seen.insert(m.mv.0.packed()));
            dropped += before - pos.moves.len();
            true
        });
        dropped
    }

    pub fn entry(&mut self, board: &Board) -> &mut BookPosition {
        self.positions.entry(Book::key(board)).or_default()
    }

    /// Add or update a move; counts add up, the other fields are overwritten when present.
    pub fn add(&mut self, board: &Board, m: BookMove) {
        let ply = board.move_number;
        let pos = self.entry(board);
        pos.ply = Some(pos.ply.map_or(ply, |p| p.min(ply)));
        match pos.find_mut(m.mv.0) {
            Some(e) => {
                e.count = e.count.saturating_add(m.count);
                e.value = m.value.or(e.value);
                e.depth = m.depth.or(e.depth);
                e.ponder = m.ponder.or(e.ponder);
                e.wins = match (e.wins, m.wins) {
                    (Some(a), Some(b)) => Some(a.saturating_add(b)),
                    (a, b) => a.or(b),
                };
            }
            None => pos.moves.push(m),
        }
    }
}

// ---------------------------------------------------------------- operations

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergePolicy {
    /// Counts add up; values/depths from the second book win where present.
    #[default]
    Sum,
    /// Keep the first book's entries for positions both books contain.
    KeepFirst,
    /// Replace with the second book's entries for positions both contain.
    PreferSecond,
}

pub fn merge(a: &mut Book, b: Book, policy: MergePolicy) {
    for (key, pos) in b.positions {
        match a.positions.get_mut(&key) {
            None => {
                a.positions.insert(key, pos);
            }
            Some(existing) => match policy {
                MergePolicy::KeepFirst => {}
                MergePolicy::PreferSecond => *existing = pos,
                MergePolicy::Sum => {
                    existing.ply = match (existing.ply, pos.ply) {
                        (Some(x), Some(y)) => Some(x.min(y)),
                        (x, y) => x.or(y),
                    };
                    for m in pos.moves {
                        match existing.find_mut(m.mv.0) {
                            Some(e) => {
                                e.count = e.count.saturating_add(m.count);
                                e.value = m.value.or(e.value);
                                e.depth = m.depth.or(e.depth);
                                e.ponder = m.ponder.or(e.ponder);
                                if e.comment.is_empty() {
                                    e.comment = m.comment;
                                }
                            }
                            None => existing.moves.push(m),
                        }
                    }
                    if existing.comment.is_empty() {
                        existing.comment = pos.comment;
                    }
                    existing.evals.extend(pos.evals);
                }
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub min_count: Option<u64>,
    /// Keep moves whose value is within this many centipawns of the position's best.
    pub max_value_drop: Option<i32>,
    /// Keep positions whose best value lies in [min, max] (side to move's view).
    pub value_range: Option<(i32, i32)>,
    pub max_ply: Option<u32>,
    /// Only positions with this side to move (0 Black, 1 White).
    pub side: Option<u8>,
    pub top_moves: Option<usize>,
    /// Drop positions left without moves.
    pub drop_empty: bool,
}

/// Apply `f`; returns how many moves were removed.
pub fn filter(book: &mut Book, f: &Filter) -> usize {
    let mut removed = 0;
    book.positions.retain(|key, pos| {
        let side = if key.split(' ').nth(1) == Some("w") { 1 } else { 0 };
        if f.side.is_some_and(|s| s != side) || f.max_ply.is_some_and(|m| pos.ply.is_some_and(|p| p > m)) {
            removed += pos.moves.len();
            return false;
        }
        let before = pos.moves.len();
        if let Some(min) = f.min_count {
            pos.moves.retain(|m| m.count >= min);
        }
        let best = pos.moves.iter().filter_map(|m| m.value).max();
        if let (Some(drop), Some(best)) = (f.max_value_drop, best) {
            pos.moves.retain(|m| m.value.is_none_or(|v| best.saturating_sub(v) <= drop));
        }
        if let (Some((lo, hi)), Some(best)) = (f.value_range, best)
            && (best < lo || best > hi) {
                pos.moves.clear();
            }
        if let Some(n) = f.top_moves {
            pos.sort_moves();
            pos.moves.truncate(n);
        }
        removed += before - pos.moves.len();
        !(f.drop_empty && pos.moves.is_empty())
    });
    removed
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub positions: usize,
    pub moves: usize,
    pub black_to_move: usize,
    pub with_values: usize,
    pub with_counts: usize,
    pub max_ply: u32,
    pub total_count: u64,
}

pub fn stats(book: &Book) -> Stats {
    let mut s = Stats { positions: book.positions.len(), ..Default::default() };
    for (k, p) in &book.positions {
        s.moves += p.moves.len();
        s.black_to_move += usize::from(k.split(' ').nth(1) == Some("b"));
        s.with_values += p.moves.iter().filter(|m| m.value.is_some()).count();
        s.with_counts += p.moves.iter().filter(|m| m.count > 1).count();
        s.total_count = p.moves.iter().fold(s.total_count, |a, m| a.saturating_add(m.count));
        s.max_ply = s.max_ply.max(p.ply.unwrap_or(0));
    }
    s
}

/// Look up a position; with `flipped`, also try the 180°-rotated position (YaneuraOu FlippedBook).
/// Keys of the book positions reachable from `roots` by following book moves.
pub fn reachable(book: &Book, roots: &[Board]) -> std::collections::HashSet<String> {
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<Board> = roots.to_vec();
    while let Some(board) = stack.pop() {
        let key = Book::key(&board);
        let Some(pos) = book.positions.get(&key) else { continue };
        if !seen.insert(key) {
            continue;
        }
        for m in &pos.moves {
            if board.legal_moves().contains(&m.mv.0) {
                stack.push(board.after_generated(m.mv.0));
            }
        }
    }
    seen
}

pub fn probe<'a>(book: &'a Book, board: &Board, flipped: bool) -> Option<(&'a BookPosition, bool)> {
    if let Some(p) = book.positions.get(&Book::key(board)) {
        return Some((p, false));
    }
    if flipped {
        return book.positions.get(&Book::key(&flip_board(board))).map(|p| (p, true));
    }
    None
}

/// Rotate 180° and swap colours (squares s → 80 − s, hands swapped, side flipped).
pub fn flip_board(b: &Board) -> Board {
    let mut squares = [0u8; 81];
    for (s, &p) in b.squares.iter().enumerate() {
        squares[80 - s] = if p == 0 { 0 } else { p ^ 16 };
    }
    Board { squares, hands: [b.hands[1], b.hands[0]], side: 1 - b.side, move_number: b.move_number }
}

pub fn flip_move(m: Move16) -> Option<Move16> {
    use sakiyomi_kifu::notation::try_make_move;
    let to = 80 - m.destination().index();
    match m.source() {
        Some(s) => try_make_move(Some(80 - s.index()), to, m.promotes(), 0),
        None => try_make_move(None, to, false, m.drop_kind()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakiyomi_kifu::shogi_core::START_SFEN;

    fn mv(s: &str) -> Move16 {
        Move16::from_usi(s).unwrap()
    }

    pub(crate) fn sample() -> Book {
        let mut book = Book { author: "test".into(), description: "sample".into(), ..Default::default() };
        let start = Board::from_sfen(START_SFEN).unwrap();
        let mut m = BookMove::new(mv("7g7f"));
        m.value = Some(40);
        m.depth = Some(20);
        m.count = 10;
        m.ponder = Some(Move16Ser(mv("3c3d")));
        book.add(&start, m);
        let mut m2 = BookMove::new(mv("2g2f"));
        m2.value = Some(30);
        m2.count = 5;
        book.add(&start, m2);
        let after = start.after_generated(mv("7g7f"));
        let mut r = BookMove::new(mv("3c3d"));
        r.value = Some(-35);
        r.count = 7;
        book.add(&after, r);
        let after2 = after.after_generated(mv("3c3d"));
        let mut b = BookMove::new(mv("8h2b+"));
        b.value = Some(50);
        book.add(&after2, b);
        book
    }

    #[test]
    fn model_operations() {
        let mut b = sample();
        assert_eq!(b.positions.len(), 3);
        assert_eq!(b.validate(), 0);
        let start = Board::from_sfen(START_SFEN).unwrap();
        b.add(&start, BookMove { count: 3, ..BookMove::new(mv("7g7f")) });
        assert_eq!(b.positions[&Book::key(&start)].moves[0].count, 13);
        let mut bad = sample();
        bad.entry(&start).moves.push(BookMove::new(mv("5a5b")));
        assert_eq!(bad.validate(), 1);

        let mut f = sample();
        assert_eq!(filter(&mut f, &Filter { min_count: Some(6), ..Default::default() }), 2);
        let mut f = sample();
        filter(&mut f, &Filter { side: Some(0), drop_empty: true, ..Default::default() });
        assert_eq!(f.positions.len(), 2);

        let mut a = sample();
        merge(&mut a, sample(), MergePolicy::Sum);
        assert_eq!(a.positions[&Book::key(&start)].moves[0].count, 20);
        let s = stats(&a);
        assert_eq!((s.positions, s.moves, s.black_to_move), (3, 4, 2));

        let flipped = flip_board(&start.after_generated(mv("7g7f")));
        assert_eq!(flipped.side, 0);
        assert_eq!(flip_move(mv("7g7f")), Some(mv("3c3d")));
        assert_eq!(flip_move(mv("P*5e")), Some(mv("P*5e")));
        let sb = sample();
        let (p, was_flipped) = probe(&sb, &flipped, true).unwrap();
        assert!(was_flipped);
        assert_eq!(p.moves[0].mv.0, mv("3c3d"));
    }
}
