//! Complete ordinary-board legal move generation, independent of cshogi.

use crate::{HAND_TYPES, Move16, ParseError, Position, Square, error};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Board {
    pub squares: [u8; 81],
    pub hands: [[u8; 7]; 2],
    pub side: u8,
    pub move_number: u32,
}

/// Excludes move number so transpositions/repetition share a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoardKey {
    pub squares: [u8; 81],
    pub hands: [[u8; 7]; 2],
    pub side: u8,
}

impl Board {
    pub fn from_sfen(sfen: &str) -> Result<Self, ParseError> {
        Ok(Self::from_position(&Position::from_sfen(sfen)?))
    }

    pub fn from_position(position: &Position) -> Self {
        Self {
            squares: std::array::from_fn(|i| position.piece_at(Square(i as u8))),
            hands: position.hands,
            side: position.side_to_move,
            move_number: position.move_number,
        }
    }

    pub fn to_position(&self) -> Position {
        let mut position = Position {
            pieces: [[crate::Bitboard::EMPTY; 15]; 2],
            occupied: [crate::Bitboard::EMPTY; 2],
            move_number: self.move_number,
            hands: self.hands,
            side_to_move: self.side,
            reserved: [0; 5],
        };
        for (index, &piece) in self.squares.iter().enumerate() {
            if piece != 0 {
                let color = (piece >> 4) as usize;
                position.pieces[color][(piece & 15) as usize].insert(Square(index as u8));
                position.occupied[color].insert(Square(index as u8));
            }
        }
        position
    }

    pub fn to_sfen(&self) -> String {
        self.to_position().to_sfen()
    }

    pub fn key(&self) -> BoardKey {
        BoardKey {
            squares: self.squares,
            hands: self.hands,
            side: self.side,
        }
    }

    pub fn king_square(&self, side: u8) -> Option<Square> {
        self.squares
            .iter()
            .position(|&piece| piece == (8 | side << 4))
            .map(|square| Square(square as u8))
    }

    pub fn in_check(&self, side: u8) -> bool {
        self.king_square(side)
            .is_none_or(|square| self.is_attacked(square, 1 - side))
    }

    pub fn is_attacked(&self, target: Square, by_side: u8) -> bool {
        self.squares.iter().enumerate().any(|(index, &piece)| {
            piece != 0 && piece >> 4 == by_side && self.attacks(index, target.index(), piece)
        })
    }

    fn attacks(&self, from: usize, to: usize, piece: u8) -> bool {
        if from == to {
            return false;
        }
        let dx = (to / 9) as i8 - (from / 9) as i8;
        let dy = (to % 9) as i8 - (from % 9) as i8;
        let forward = if piece >> 4 == 0 { -1 } else { 1 };
        let y = dy * forward;
        let kind = piece & 15;
        let step = match kind {
            1 => dx == 0 && y == 1,
            3 => dx.abs() == 1 && y == 2,
            4 => (y == 1 && dx.abs() <= 1) || (y == -1 && dx.abs() == 1),
            7 | 9..=12 => {
                (y == 1 && dx.abs() <= 1) || (y == 0 && dx.abs() == 1) || (y == -1 && dx == 0)
            }
            8 => dx.abs() <= 1 && dy.abs() <= 1,
            13 => dx.abs() + dy.abs() == 1,
            14 => dx.abs() == 1 && dy.abs() == 1,
            _ => false,
        };
        if step {
            return true;
        }
        let sliding = match kind {
            2 => dx == 0 && y > 0,
            5 | 13 => dx.abs() == dy.abs(),
            6 | 14 => dx == 0 || dy == 0,
            _ => false,
        };
        if !sliding {
            return false;
        }
        let mut file = (from / 9) as i8 + dx.signum();
        let mut rank = (from % 9) as i8 + dy.signum();
        while file != (to / 9) as i8 || rank != (to % 9) as i8 {
            if self.squares[(file * 9 + rank) as usize] != 0 {
                return false;
            }
            file += dx.signum();
            rank += dy.signum();
        }
        true
    }

    fn board_moves(&self) -> Vec<Move16> {
        let mut moves = Vec::with_capacity(128);
        for (from, &piece) in self.squares.iter().enumerate() {
            if piece == 0 || piece >> 4 != self.side {
                continue;
            }
            let kind = piece & 15;
            for to in 0..81 {
                let occupant = self.squares[to];
                if occupant != 0 && (occupant >> 4 == self.side || occupant & 15 == 8) {
                    continue;
                }
                if !self.attacks(from, to, piece) {
                    continue;
                }
                let encoded = ((from as u16) << 7) | to as u16;
                let promotable =
                    kind <= 6 && (in_zone(self.side, from % 9) || in_zone(self.side, to % 9));
                if promotable {
                    moves.push(Move16(encoded | (1 << 14)));
                }
                if !dead_square(kind, self.side, to % 9) {
                    moves.push(Move16(encoded));
                }
            }
        }
        moves
    }

    fn pseudo_moves(&self) -> Vec<Move16> {
        let mut moves = self.board_moves();
        let pawns = std::array::from_fn::<_, 9, _>(|file| {
            self.squares[file * 9..file * 9 + 9].contains(&(1 | self.side << 4))
        });
        for (hand, &kind) in HAND_TYPES.iter().enumerate() {
            if self.hands[self.side as usize][hand] == 0 {
                continue;
            }
            for to in 0..81 {
                if self.squares[to] != 0
                    || dead_square(kind, self.side, to % 9)
                    || (kind == 1 && pawns[to / 9])
                {
                    continue;
                }
                moves.push(Move16(((kind as u16 + 80) << 7) | to as u16));
            }
        }
        moves
    }

    pub fn legal_moves(&self) -> Vec<Move16> {
        self.pseudo_moves()
            .into_iter()
            .filter(|&mv| {
                let child = self.after_generated(mv);
                if child.in_check(self.side) {
                    return false;
                }
                if mv.drop_kind() == Some(1) {
                    // A direct pawn check cannot be blocked by dropping another piece.
                    // Evasions are king moves or board moves capturing the pawn.
                    let pawn = mv.destination().index();
                    let checked = child
                        .king_square(child.side)
                        .is_some_and(|king| child.attacks(pawn, king.index(), 1 | self.side << 4));
                    if checked
                        && !child
                            .board_moves()
                            .into_iter()
                            .any(|reply| !child.after_generated(reply).in_check(child.side))
                    {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    pub fn checking_moves(&self) -> Vec<Move16> {
        self.legal_moves()
            .into_iter()
            .filter(|&mv| self.after_generated(mv).in_check(1 - self.side))
            .collect()
    }

    /// Apply a move only after checking it against the full legal move set.
    pub fn play(&self, mv: Move16) -> Result<Self, ParseError> {
        if !self.legal_moves().contains(&mv) {
            return Err(error("Illegal move"));
        }
        Ok(self.after_generated(mv))
    }

    /// Fast path for moves produced by legal_moves; invalid input is a programmer error.
    pub fn after_generated(&self, mv: Move16) -> Self {
        let mut next = *self;
        let to = mv.destination().index();
        let kind = if let Some(source) = mv.source() {
            let from = source.index();
            let kind = next.squares[from] & 15;
            next.squares[from] = 0;
            let captured = next.squares[to] & 15;
            if captured != 0 {
                let base = if captured > 8 { captured - 8 } else { captured };
                let hand = HAND_TYPES
                    .iter()
                    .position(|&p| p == base)
                    .expect("King capture");
                next.hands[self.side as usize][hand] += 1;
            }
            kind + if mv.promotes() { 8 } else { 0 }
        } else {
            let kind = mv.drop_kind().expect("Drop kind");
            let hand = HAND_TYPES
                .iter()
                .position(|&p| p == kind)
                .expect("Invalid drop");
            next.hands[self.side as usize][hand] -= 1;
            kind
        };
        next.squares[to] = kind | self.side << 4;
        next.side = 1 - self.side;
        next.move_number += 1;
        next
    }

    pub fn perft(&self, depth: u8) -> u64 {
        if depth == 0 {
            return 1;
        }
        let moves = self.legal_moves();
        if depth == 1 {
            return moves.len() as u64;
        }
        moves
            .into_iter()
            .map(|mv| self.after_generated(mv).perft(depth - 1))
            .sum()
    }

    pub fn declaration_win(&self) -> bool {
        let Some(king) = self.king_square(self.side) else {
            return false;
        };
        if !in_zone(self.side, king.index() % 9) || self.in_check(self.side) {
            return false;
        }
        let mut camp_count = 0;
        let mut points = 0u16;
        for (square, &piece) in self.squares.iter().enumerate() {
            if piece == 0
                || piece >> 4 != self.side
                || piece & 15 == 8
                || !in_zone(self.side, square % 9)
            {
                continue;
            }
            camp_count += 1;
            points += piece_points(piece & 15);
        }
        for (index, &number) in self.hands[self.side as usize].iter().enumerate() {
            points += number as u16 * piece_points(HAND_TYPES[index]);
        }
        camp_count >= 10 && points >= if self.side == 0 { 28 } else { 27 }
    }
}

fn piece_points(kind: u8) -> u16 {
    if matches!(kind, 5 | 6 | 13 | 14) {
        5
    } else {
        1
    }
}

fn in_zone(side: u8, rank: usize) -> bool {
    if side == 0 { rank < 3 } else { rank >= 6 }
}

fn dead_square(kind: u8, side: u8, rank: usize) -> bool {
    let distance = if side == 0 { rank } else { 8 - rank };
    matches!(kind, 1 | 2) && distance == 0 || kind == 3 && distance < 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_perft() {
        let board = Board::from_sfen(crate::START_SFEN).unwrap();
        assert_eq!(board.perft(0), 1);
        assert_eq!(board.perft(1), 30);
        assert_eq!(board.perft(2), 900);
        assert_eq!(board.perft(3), 25470);
        assert_eq!(board.to_sfen(), crate::START_SFEN);
    }

    #[test]
    fn apply_capture_promote_drop_and_check() {
        let board = Board::from_sfen("4k4/9/P8/9/9/9/9/9/4K4 b P 1").unwrap();
        let next = board.play(Move16::from_usi("9c9b+").unwrap()).unwrap();
        assert_eq!(next.squares[Square::from_usi("9b").unwrap().index()], 9);
        assert!(board.play(Move16::from_usi("P*9e").unwrap()).is_err());
        assert_eq!(board.perft(2), 345);
    }

    #[test]
    fn pawn_drop_mate_is_forbidden_but_rook_drop_mate_is_legal() {
        let board = Board::from_sfen("8k/6G2/7G1/9/9/9/9/9/K8 b PR 1").unwrap();
        let moves = board.legal_moves();
        assert!(!moves.contains(&Move16::from_usi("P*1b").unwrap()));
        let mate = Move16::from_usi("R*1b").unwrap();
        assert!(moves.contains(&mate));
        let child = board.after_generated(mate);
        assert!(child.in_check(1));
        assert!(child.legal_moves().is_empty());
    }
}
