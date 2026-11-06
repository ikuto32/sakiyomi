//! Minimax back-up over a book graph ("peta shock") and leaf selection ("peta next"),
//! following the semantics of YaneuraOu's `makebook peta_shock` and BookMiner's PetaNext:
//!
//! * A move whose resulting position is in the book gets `value = −best(child)`,
//!   `depth = best(child).depth + 1`; other moves keep their searched value (depth 0).
//! * Positions are matched as-is or 180°-flipped (FlippedBook), so one side of each mirrored
//!   pair is enough.
//! * Leaf selection walks from the root: on the book side's turn only the best move, on the
//!   opponent's turn every move within `eval_diff` of the root's best value (an absolute
//!   bound, so lines cannot drift). Positions not in the book are the leaves to search next.

use std::collections::{HashMap, HashSet, VecDeque};

use sakiyomi_kifu::shogi_core::Move16;
use sakiyomi_kifu::shogi_core::rules::Board;

use crate::{Book, flip_board};

pub const MAX_DEPTH: u32 = 9999;
/// Value assigned to positions without any valued move.
const UNKNOWN: i32 = i32::MIN / 4;

/// Key of the book entry for `board` (as-is or flipped), if any.
fn lookup(book: &Book, board: &Board) -> Option<String> {
    let k = Book::key(board);
    if book.positions.contains_key(&k) {
        return Some(k);
    }
    let f = Book::key(&flip_board(board));
    book.positions.contains_key(&f).then_some(f)
}

/// Back up values through the book graph in place. Returns the number of passes used.
pub fn back_up(book: &mut Book, max_passes: usize) -> usize {
    // Edges: (position key, move index) → child key.
    let keys: Vec<String> = book.positions.keys().cloned().collect();
    let index: HashMap<&str, usize> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut edges: Vec<Vec<Option<usize>>> = Vec::with_capacity(keys.len());
    let mut leaf: Vec<Vec<(i32, u32)>> = Vec::with_capacity(keys.len());
    for k in &keys {
        let pos = &book.positions[k];
        let board = Book::board(k);
        edges.push(
            pos.moves
                .iter()
                .map(|m| board.and_then(|b| lookup(book, &b.after_generated(m.mv.0))).and_then(|c| index.get(c.as_str()).copied()))
                .collect(),
        );
        leaf.push(pos.moves.iter().map(|m| (m.value.unwrap_or(UNKNOWN), m.depth.unwrap_or(0))).collect());
    }
    // best[i] = (value, depth) of position i, iterated to a fixed point.
    let mut best: Vec<(i32, u32)> = leaf.iter().map(|ms| ms.iter().copied().max_by_key(|&(v, _)| v).unwrap_or((UNKNOWN, 0))).collect();
    let mut passes = 0;
    for _ in 0..max_passes {
        passes += 1;
        let mut changed = false;
        for i in 0..keys.len() {
            let mut b = (UNKNOWN, 0u32);
            for (j, e) in edges[i].iter().enumerate() {
                let v = match e {
                    Some(c) if best[*c].0 > UNKNOWN => (-best[*c].0, (best[*c].1 + 1).min(MAX_DEPTH)),
                    _ => leaf[i][j],
                };
                // Prefer higher values; at equal value prefer the shorter line when winning,
                // the longer one when losing (as peta_shock does).
                let better = v.0 > b.0 || (v.0 == b.0 && if v.0 >= 0 { v.1 < b.1 } else { v.1 > b.1 });
                if better {
                    b = v;
                }
            }
            if b != best[i] {
                best[i] = b;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (i, k) in keys.iter().enumerate() {
        let Some(pos) = book.positions.get_mut(k) else { continue };
        for (j, m) in pos.moves.iter_mut().enumerate() {
            if let Some(c) = edges[i][j].filter(|&c| best[c].0 > UNKNOWN) {
                m.value = Some(-best[c].0);
                m.depth = Some((best[c].1 + 1).min(MAX_DEPTH));
            }
        }
    }
    passes
}

/// A leaf to search: the moves from `root` that lead to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub root: Board,
    pub moves: Vec<Move16>,
}

/// Leaves of the book as seen by `book_side` (0 Black, 1 White) from `root`.
pub fn leaves(book: &Book, root: &Board, book_side: u8, eval_diff: i32, max_step: usize, limit: usize) -> Vec<Leaf> {
    let Some(root_key) = lookup(book, root) else {
        return vec![Leaf { root: *root, moves: vec![] }];
    };
    let root_best = book.positions[&root_key].moves.iter().filter_map(|m| m.value).max().unwrap_or(0);
    // Root best from the book side's point of view.
    let root_value = if root.side == book_side { root_best } else { -root_best };
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(Board, Vec<Move16>)> = VecDeque::from([(*root, Vec::new())]);
    while let Some((board, line)) = queue.pop_front() {
        if out.len() >= limit {
            break;
        }
        let Some(key) = lookup(book, &board) else {
            let norm = Book::key(&board).min(Book::key(&flip_board(&board)));
            if seen.insert(norm) {
                out.push(Leaf { root: *root, moves: line });
            }
            continue;
        };
        if !seen.insert(key.clone()) || line.len() >= max_step {
            continue;
        }
        let pos = &book.positions[&key];
        let flipped = key != Book::key(&board);
        let real = |m: Move16| if flipped { crate::flip_move(m) } else { Some(m) };
        let mut moves: Vec<(Move16, i32)> = pos.moves.iter().filter_map(|m| Some((real(m.mv.0)?, m.value?))).collect();
        moves.sort_by_key(|m| std::cmp::Reverse(m.1));
        let follow: Vec<Move16> = if board.side == book_side {
            moves.first().map(|m| vec![m.0]).unwrap_or_default()
        } else {
                        // The opponent's best is about −root_value (their view); follow replies within eval_diff of it.
            moves.iter().filter(|(_, v)| *v >= -root_value - eval_diff).map(|m| m.0).collect()
        };
        for m in follow {
            if !board.legal_moves().contains(&m) {
                continue;
            }
            let mut l = line.clone();
            l.push(m);
            queue.push_back((board.after_generated(m), l));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BookMove, Move16Ser};
    use sakiyomi_kifu::shogi_core::START_SFEN;

    fn mv(s: &str) -> Move16 {
        Move16::from_usi(s).unwrap()
    }

    fn valued(m: &str, v: i32) -> BookMove {
        BookMove { value: Some(v), depth: Some(0), ..BookMove::new(mv(m)) }
    }

    #[test]
    fn back_up_propagates_minimax() {
        let start = Board::from_sfen(START_SFEN).unwrap();
        let mut book = Book::default();
        book.add(&start, valued("7g7f", 50));
        book.add(&start, valued("2g2f", 40));
        let a = start.after_generated(mv("7g7f"));
        // After 7g7f White's best reply is only −120 for White → 7g7f is worth +120 to Black.
        book.add(&a, valued("3c3d", -120));
        book.add(&a, valued("8c8d", -200));
        back_up(&mut book, 50);
        let root = &book.positions[&Book::key(&start)];
        let m = root.moves.iter().find(|m| m.mv.0 == mv("7g7f")).unwrap();
        assert_eq!((m.value, m.depth), (Some(120), Some(1)));
        let other = root.moves.iter().find(|m| m.mv.0 == mv("2g2f")).unwrap();
        assert_eq!(other.value, Some(40));
    }

    #[test]
    fn leaf_selection_follows_best_and_near_best_replies() {
        let start = Board::from_sfen(START_SFEN).unwrap();
        let mut book = Book::default();
        book.add(&start, valued("7g7f", 50));
        book.add(&start, valued("2g2f", 10));
        let a = start.after_generated(mv("7g7f"));
        book.add(&a, valued("3c3d", -50));
        book.add(&a, valued("8c8d", -70));
        book.add(&a, valued("1c1d", -300));
        let black = leaves(&book, &start, 0, 30, 100, 100);
        let lines: Vec<String> = black.iter().map(|l| l.moves.iter().map(|m| m.to_usi()).collect::<Vec<_>>().join(" ")).collect();
        assert!(lines.contains(&"7g7f 3c3d".to_owned()), "{lines:?}");
        assert!(lines.contains(&"7g7f 8c8d".to_owned()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("1c1d") || l.starts_with("2g2f")), "{lines:?}");
        // Flipped lookup: a White-to-move position stored as its Black mirror is found.
        let mut mirror = Book::default();
        mirror.add(&flip_board(&a), valued("7g7f", 10));
        assert!(lookup(&mirror, &a).is_some());
    }

    #[test]
    fn cycles_terminate() {
        // Two positions pointing at each other (a repetition) must not loop forever.
        let b = Board::from_sfen("4k4/9/9/9/9/9/9/9/4K4 b - 1").unwrap();
        let mut book = Book::default();
        book.add(&b, valued("5i5h", 0));
        let c = b.after_generated(mv("5i5h"));
        book.add(&c, valued("5a5b", 0));
        let d = c.after_generated(mv("5a5b"));
        book.add(&d, valued("5h5i", 0));
        let e = d.after_generated(mv("5h5i"));
        book.add(&e, valued("5b5a", 0));
        let passes = back_up(&mut book, 30);
        assert!(passes <= 30);
        let _ = Move16Ser(mv("5i5h"));
    }
}
