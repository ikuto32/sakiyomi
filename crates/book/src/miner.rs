//! Book mining (BookMiner-style): repeatedly back up the book, pick leaf positions along
//! the best lines, search them with engines, and store the results.
//!
//! The book passed in is the *raw* book: values are the engines' leaf evaluations (depth 0).
//! Export it as is to continue mining later, or run [`crate::peta::back_up`] on a copy for
//! a playable book.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

use sakiyomi_kifu::shogi_core::Move16;
use sakiyomi_kifu::shogi_core::rules::Board;

use crate::peta::{self, Leaf};
use crate::{Book, BookMove, flip_board};

/// One PV line of a search: first move and its score from the side to move's view
/// (mate in n mapped to ±(32000 − n), as YaneuraOu books store it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLine {
    pub mv: Move16,
    pub value: i32,
}

/// An engine that can search a position with MultiPV and a node budget.
pub trait Searcher: Send {
    fn search(&mut self, root: &Board, moves: &[Move16], multipv: usize, nodes: u64) -> Result<Vec<SearchLine>, String>;
}

#[derive(Clone, Debug)]
pub struct MinerConfig {
    pub nodes: u64,
    /// Initial MultiPV (BookMiner uses 4).
    pub multipv: usize,
    /// Widen MultiPV while the top and last lines are within this many centipawns.
    pub multipv_delta: i32,
    /// Leaf selection: opponent moves within this of the root's best value are followed.
    pub eval_diff: i32,
    /// Do not search positions beyond this ply.
    pub max_ply: u32,
    pub max_step: usize,
    /// Leaves searched per round (per root and side).
    pub leaves_per_round: usize,
    /// Rounds to run (0 = until no leaves remain or cancelled).
    pub rounds: usize,
    /// Mine for Black's book and/or White's book.
    pub sides: [bool; 2],
}

impl Default for MinerConfig {
    fn default() -> Self {
        MinerConfig {
            nodes: 1_000_000,
            multipv: 4,
            multipv_delta: 100,
            eval_diff: 30,
            max_ply: 200,
            max_step: 1000,
            leaves_per_round: 64,
            rounds: 0,
            sides: [true, true],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinerProgress {
    pub round: usize,
    pub searched: usize,
    pub queued: usize,
    pub positions: usize,
    pub errors: usize,
}

/// Search one position, widening MultiPV while the candidates are close (BookMiner rule).
pub fn search_widening(s: &mut dyn Searcher, root: &Board, moves: &[Move16], board: &Board, cfg: &MinerConfig) -> Result<Vec<SearchLine>, String> {
    let legal = board.legal_moves().len();
    if legal == 0 {
        return Ok(Vec::new());
    }
    let mut k = cfg.multipv.max(1).min(legal);
    let mut nodes = cfg.nodes.max(1);
    let mut best: Vec<SearchLine> = Vec::new();
    loop {
        let mut lines = s.search(root, moves, k, nodes)?;
        lines.retain(|l| board.legal_moves().contains(&l.mv));
        for l in lines {
            match best.iter_mut().find(|b| b.mv == l.mv) {
                Some(b) => *b = l,
                None => best.push(l),
            }
        }
        best.sort_by_key(|l| std::cmp::Reverse(l.value));
        let spread = match (best.first(), best.get(k.saturating_sub(1))) {
            (Some(a), Some(b)) => a.value.saturating_sub(b.value),
            _ => i32::MAX,
        };
        if best.len() < k || k >= legal || spread > cfg.multipv_delta {
            return Ok(best);
        }
        k = (k + 4).min(legal);
        nodes = (nodes / 2).max(1);
    }
}

fn replay(leaf: &Leaf) -> Option<Board> {
    let mut b = leaf.root;
    for &m in &leaf.moves {
        if !b.legal_moves().contains(&m) {
            return None;
        }
        b = b.after_generated(m);
    }
    Some(b)
}

/// Store search results as leaf values (overwriting earlier values of the same moves).
pub fn store(book: &mut Book, board: &Board, lines: &[SearchLine]) {
    let pos = book.entry(board);
    pos.ply = Some(pos.ply.map_or(board.move_number, |p| p.min(board.move_number)));
    for l in lines {
        match pos.find_mut(l.mv) {
            Some(m) => {
                m.value = Some(l.value);
                m.depth = Some(0);
            }
            None => pos.moves.push(BookMove { value: Some(l.value), depth: Some(0), ..BookMove::new(l.mv) }),
        }
    }
    pos.moves.sort_by_key(|l| std::cmp::Reverse(l.value));
}

/// Run the mining loop until finished or `cancel` is set.
pub fn mine(
    book: &mut Book,
    roots: &[Board],
    searchers: Vec<Box<dyn Searcher>>,
    cfg: &MinerConfig,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&MinerProgress),
    mut autosave: impl FnMut(&Book),
) -> MinerProgress {
    let mut p = MinerProgress { positions: book.positions.len(), ..Default::default() };
    let workers: Vec<Mutex<Box<dyn Searcher>>> = searchers.into_iter().map(Mutex::new).collect();
    if workers.is_empty() {
        return p;
    }
    loop {
        if cancel.load(Ordering::Relaxed) || (cfg.rounds > 0 && p.round >= cfg.rounds) {
            break;
        }
        p.round += 1;
        let mut working = book.clone();
        peta::back_up(&mut working, 64);
        let mut seen = HashSet::new();
        let mut queue: VecDeque<Leaf> = VecDeque::new();
        for root in roots {
            for side in 0..2u8 {
                if !cfg.sides[side as usize] {
                    continue;
                }
                for leaf in peta::leaves(&working, root, side, cfg.eval_diff, cfg.max_step, cfg.leaves_per_round) {
                    let Some(b) = replay(&leaf) else { continue };
                    if b.move_number > cfg.max_ply || b.legal_moves().is_empty() {
                        continue;
                    }
                    let norm = Book::key(&b).min(Book::key(&flip_board(&b)));
                    if seen.insert(norm) {
                        queue.push_back(leaf);
                    }
                }
            }
        }
        if queue.is_empty() {
            break;
        }
        p.queued = queue.len();
        progress(&p);
        let queue = Mutex::new(queue);
        let (tx, rx) = mpsc::channel::<(Board, Result<Vec<SearchLine>, String>)>();
        std::thread::scope(|scope| {
            for w in &workers {
                let tx = tx.clone();
                let queue = &queue;
                scope.spawn(move || {
                    let mut s = match w.lock() {
                        Ok(s) => s,
                        Err(e) => e.into_inner(),
                    };
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let Some(leaf) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { return };
                        let Some(board) = replay(&leaf) else { continue };
                        let r = search_widening(&mut **s, &leaf.root, &leaf.moves, &board, cfg);
                        if tx.send((board, r)).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(tx);
            for (board, r) in rx {
                match r {
                    Ok(lines) => store(book, &board, &lines),
                    Err(_) => p.errors += 1,
                }
                p.searched += 1;
                p.queued = p.queued.saturating_sub(1);
                p.positions = book.positions.len();
                progress(&p);
            }
        });
        autosave(book);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakiyomi_kifu::shogi_core::START_SFEN;

    /// Deterministic fake engine: scores moves by a hash of the move text.
    struct Fake {
        calls: usize,
    }

    impl Searcher for Fake {
        fn search(&mut self, root: &Board, moves: &[Move16], multipv: usize, _nodes: u64) -> Result<Vec<SearchLine>, String> {
            self.calls += 1;
            let mut b = *root;
            for &m in moves {
                b = b.after_generated(m);
            }
            let mut lines: Vec<SearchLine> = b
                .legal_moves()
                .into_iter()
                .map(|m| SearchLine { mv: m, value: (m.packed() as i32 * 7919) % 200 - 100 })
                .collect();
            lines.sort_by_key(|l| std::cmp::Reverse(l.value));
            lines.truncate(multipv);
            Ok(lines)
        }
    }

    #[test]
    fn mining_grows_the_book_and_stops() {
        let start = Board::from_sfen(START_SFEN).unwrap();
        let mut book = Book::default();
        let cancel = AtomicBool::new(false);
        let cfg = MinerConfig { rounds: 4, leaves_per_round: 8, eval_diff: 20, ..Default::default() };
        let mut saves = 0;
        let p = mine(&mut book, &[start], vec![Box::new(Fake { calls: 0 }), Box::new(Fake { calls: 0 })], &cfg, &cancel, |_| {}, |_| saves += 1);
        assert_eq!(p.round, 4);
        assert!(p.searched >= 4, "{p:?}");
        assert!(book.positions.len() >= 4);
        assert_eq!(saves, 4);
        assert_eq!(book.validate(), 0);
        // The mined book is a valid YaneuraOu book after back-up.
        let mut playable = book.clone();
        peta::back_up(&mut playable, 64);
        let bytes = crate::db::write(&playable);
        assert!(crate::db::read(&bytes, &Default::default()).is_ok());
    }

    #[test]
    fn widening_and_cancel() {
        struct Flat;
        impl Searcher for Flat {
            fn search(&mut self, root: &Board, moves: &[Move16], multipv: usize, _: u64) -> Result<Vec<SearchLine>, String> {
                let mut b = *root;
                for &m in moves {
                    b = b.after_generated(m);
                }
                Ok(b.legal_moves().into_iter().take(multipv).map(|m| SearchLine { mv: m, value: 0 }).collect())
            }
        }
        let start = Board::from_sfen(START_SFEN).unwrap();
        // All moves equal: MultiPV widens until every legal move is covered.
        let lines = search_widening(&mut Flat, &start, &[], &start, &MinerConfig::default()).unwrap();
        assert_eq!(lines.len(), 30);
        let cancel = AtomicBool::new(true);
        let p = mine(&mut Book::default(), &[start], vec![Box::new(Flat)], &MinerConfig::default(), &cancel, |_| {}, |_| {});
        assert_eq!(p.round, 0);
    }
}
