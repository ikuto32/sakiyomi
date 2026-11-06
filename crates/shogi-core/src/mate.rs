//! Bounded checking-move OR / all-reply AND proof search.

use crate::Move16;
use crate::rules::{Board, BoardKey};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProofStatus {
    Proven,
    DisprovenWithinDepth,
    Unknown,
}

#[derive(Debug)]
pub struct MateResult {
    pub status: ProofStatus,
    pub nodes: u64,
    /// Representative longest defensive line; the search covers every defense.
    pub pv: Vec<Move16>,
}

pub fn prove_mate(
    board: &Board,
    max_plies: u8,
    node_limit: u64,
    deadline: Option<Instant>,
    stop: &AtomicBool,
) -> MateResult {
    assert!(max_plies > 0 && max_plies % 2 == 1 && node_limit > 0);
    let mut solver = Solver {
        attacker: board.side,
        nodes: 0,
        node_limit,
        deadline,
        stop,
        path: HashSet::new(),
    };
    let (status, pv) = solver.visit(board, max_plies);
    MateResult {
        status,
        nodes: solver.nodes,
        pv,
    }
}

struct Solver<'a> {
    attacker: u8,
    nodes: u64,
    node_limit: u64,
    deadline: Option<Instant>,
    stop: &'a AtomicBool,
    path: HashSet<BoardKey>,
}

impl Solver<'_> {
    fn visit(&mut self, board: &Board, remaining: u8) -> (ProofStatus, Vec<Move16>) {
        use ProofStatus::*;
        if self.nodes >= self.node_limit
            || self.stop.load(Ordering::Relaxed)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return (Unknown, vec![]);
        }
        self.nodes += 1;
        let key = board.key();
        if self.path.contains(&key) {
            return (DisprovenWithinDepth, vec![]);
        }
        let moves = board.legal_moves();
        let attack = board.side == self.attacker;
        if moves.is_empty() {
            return (
                if !attack && board.in_check(board.side) {
                    Proven
                } else {
                    DisprovenWithinDepth
                },
                vec![],
            );
        }
        if remaining == 0 {
            return (DisprovenWithinDepth, vec![]);
        }
        self.path.insert(key);
        let mut unknown = false;
        let mut longest = vec![];
        for mv in moves {
            let child = board.after_generated(mv);
            if attack && !child.in_check(child.side) {
                continue;
            }
            let (status, mut pv) = self.visit(&child, remaining - 1);
            if status == Unknown {
                unknown = true;
            }
            if attack && status == Proven {
                self.path.remove(&key);
                pv.insert(0, mv);
                return (Proven, pv);
            }
            if !attack && status == DisprovenWithinDepth {
                self.path.remove(&key);
                return (DisprovenWithinDepth, vec![]);
            }
            if !attack && status == Proven && pv.len() + 1 > longest.len() {
                pv.insert(0, mv);
                longest = pv;
            }
        }
        self.path.remove(&key);
        (
            if unknown {
                Unknown
            } else if attack {
                DisprovenWithinDepth
            } else {
                Proven
            },
            longest,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_and_exhaustion_are_distinct() {
        let board = Board::from_sfen("8k/9/7G1/9/9/9/9/7R1/K8 b R 1").unwrap();
        let stop = AtomicBool::new(false);
        let proof = prove_mate(&board, 1, 10000, None, &stop);
        assert_eq!(proof.status, ProofStatus::Proven);
        let mut end = board;
        for mv in proof.pv {
            end = end.play(mv).unwrap();
        }
        assert!(end.in_check(end.side) && end.legal_moves().is_empty());
        assert_eq!(
            prove_mate(&board, 3, 1, None, &stop).status,
            ProofStatus::Unknown
        );
        let initial = Board::from_sfen(crate::START_SFEN).unwrap();
        assert_eq!(
            prove_mate(&initial, 1, 10000, None, &stop).status,
            ProofStatus::DisprovenWithinDepth
        );
    }
}
