//! Conversions between SPG and Sakiyomi's models (SPG v1.0 §10): game records
//! ([`sakiyomi_kifu::record::GameRecord`]) and opening books ([`sakiyomi_book::Book`]).
//!
//! Importing keeps everything the source format says (lossless level L1 of §10.1); writing
//! back reports, never silently drops, what the target model cannot hold (§10.5).

pub mod book;
pub mod record;

use std::path::Path;

use sakiyomi_kifu::record::parse_sfen;
use sakiyomi_kifu::shogi_core::Move16;
use sakiyomi_kifu::shogi_core::rules::Board;
use spg::{Move, Position, ReadOptions, Spg};

pub use book::{BookSource, Merge, book_to_spg, merged_books_to_spg, spg_to_book};
pub use record::{record_to_spg, records_to_spg, spg_to_records};

/// Warnings collected while writing SPG data back into a model that holds less.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lost(pub Vec<(String, usize)>);

impl Lost {
    pub fn add(&mut self, what: &str, n: usize) {
        if n == 0 {
            return;
        }
        match self.0.iter_mut().find(|(w, _)| w == what) {
            Some((_, c)) => *c += n,
            None => self.0.push((what.to_owned(), n)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn summary(&self) -> String {
        self.0.iter().map(|(w, n)| format!("{w} ×{n}")).collect::<Vec<_>>().join(", ")
    }
}

pub fn board_to_position(b: &Board) -> Position {
    let mut p = Position::empty(if b.side == 0 { spg::Color::Black } else { spg::Color::White });
    for (sq, &code) in b.squares.iter().enumerate() {
        if code == 0 {
            continue;
        }
        let k = code & 15;
        let (base, promoted) = if k > 8 { (k - 8, true) } else { (k, false) };
        let kind = match base {
            1 => spg::Kind::Pawn,
            2 => spg::Kind::Lance,
            3 => spg::Kind::Knight,
            4 => spg::Kind::Silver,
            5 => spg::Kind::Bishop,
            6 => spg::Kind::Rook,
            7 => spg::Kind::Gold,
            _ => spg::Kind::King,
        };
        let color = if code >> 4 == 0 { spg::Color::Black } else { spg::Color::White };
        p.board[sq] = Some(spg::Piece::new(kind, color, promoted));
    }
    // shogi-core hand order is P L N S G B R, the SPG code order.
    for c in 0..2 {
        for k in 0..7 {
            p.hands[c][k] = u32::from(b.hands[c][k]);
        }
    }
    p
}

/// A board for the GUI's rules, which need both kings and the standard set of pieces.
pub fn position_to_board(p: &Position, move_number: u32) -> Option<Board> {
    parse_sfen(&format!("{} {}", p.to_sfen(), move_number.max(1))).ok()
}

pub fn move_from_core(m: Move16) -> Move {
    // Drop codes differ (cshogi: B R G; SPG: G B R); USI is the common ground.
    Move::from_usi(&m.to_usi()).unwrap_or(Move::NONE)
}

pub fn move_to_core(m: Move) -> Option<Move16> {
    Move16::from_usi(&m.to_usi()).ok()
}

/// Read `.spg`, `.spgt` or `.spgm` (by content; manifests by path).
pub fn read_path(path: &Path, limit: u64, opts: &ReadOptions) -> Result<Spg, String> {
    if path.extension().is_some_and(|e| e == "spgm") || path.to_string_lossy().ends_with(".spgm.bak") {
        return spg::manifest::read_set(path, opts, limit).map(|(s, _)| s).map_err(|e| e.to_string());
    }
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > limit {
        return Err(format!("{} is larger than {limit} bytes", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    read_bytes(&bytes, opts)
}

pub fn read_bytes(bytes: &[u8], opts: &ReadOptions) -> Result<Spg, String> {
    match spg::sniff(bytes) {
        Some("spg") => spg::read(bytes, opts).map(|(s, _)| s).map_err(|e| e.to_string()),
        Some("spgt") => {
            let text = std::str::from_utf8(bytes).map_err(|_| "SPG text must be UTF-8".to_owned())?;
            spg::text::parse(text).map_err(|e| e.to_string())
        }
        Some(_) => Err("open the .spgm manifest by its path".into()),
        None => Err("not an SPG file".into()),
    }
}

/// `.spg` bytes, or canonical `.spgt` text when `text` is set.
pub fn to_bytes(s: &Spg, text: bool) -> Result<Vec<u8>, String> {
    if text {
        spg::text::write(s).map(String::into_bytes).map_err(|e| e.to_string())
    } else {
        spg::write(s, &spg::WriteOptions::default()).map(|(b, _)| b).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use sakiyomi_kifu::record::start_board;

    #[test]
    fn boards_and_moves() {
        let b = start_board();
        let p = board_to_position(&b);
        assert_eq!(p, Position::startpos());
        assert_eq!(position_to_board(&p, 1).unwrap(), b);
        for u in ["7g7f", "8h2b+", "G*5e", "B*5e", "R*1a", "P*5e"] {
            let m = Move16::from_usi(u).unwrap();
            assert_eq!(move_from_core(m).to_usi(), u);
            assert_eq!(move_to_core(move_from_core(m)), Some(m));
        }
        let hand = Board::from_sfen("lnsgkgsnl/1r5+B1/pppppp1pp/6p2/9/2P6/PP1PPPPPP/7R1/LNSGKGSNL w B 4").unwrap();
        assert_eq!(board_to_position(&hand).to_sfen(), "lnsgkgsnl/1r5+B1/pppppp1pp/6p2/9/2P6/PP1PPPPPP/7R1/LNSGKGSNL w B");
    }
}

#[cfg(test)]
mod cross_check {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use sakiyomi_kifu::record::start_board;

    /// The SPG move rules (§6.7) and shogi-core's generator are independent; on positions
    /// with both kings they must agree move for move.
    #[test]
    fn legal_moves_agree_with_shogi_core() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut checked = 0;
        for _ in 0..60 {
            let mut b = start_board();
            for _ in 0..160 {
                let core: Vec<Move16> = b.legal_moves();
                let mut ours: Vec<String> = board_to_position(&b).legal_moves().iter().map(|m| m.to_usi()).collect();
                let mut theirs: Vec<String> = core.iter().map(|m| m.to_usi()).collect();
                ours.sort();
                theirs.sort();
                assert_eq!(ours, theirs, "{}", b.to_sfen());
                checked += 1;
                if core.is_empty() {
                    break;
                }
                b = b.after_generated(core[(rnd() % core.len() as u64) as usize]);
            }
        }
        assert!(checked > 1000);
    }
}
