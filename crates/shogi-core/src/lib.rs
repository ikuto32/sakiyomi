//! Stable square/piece encodings and an 81-bit board layout, matching cshogi.
//! CPU rules and proof search are the correctness reference for GPU kernels.

pub mod mate;
pub mod rules;

use std::fmt;
use std::ops::{BitAnd, BitOr, Not};

pub const START_SFEN: &str = "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1";
const HIGH_MASK: u64 = (1 << 17) - 1;
// cshogi piece IDs: pawn, lance, knight, silver, bishop, rook, gold, king.
const PIECES: &[u8; 8] = b"PLNSBRGK";
const MAX_MATERIAL: [u16; 8] = [18, 4, 4, 4, 2, 2, 4, 2];
// Hands use pawn, lance, knight, silver, gold, bishop, rook (cshogi H* IDs).
const HAND_TYPES: [u8; 7] = [1, 2, 3, 4, 7, 5, 6];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ParseError {}

fn error(message: &str) -> ParseError {
    ParseError(message.to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Square(u8);

impl Square {
    pub fn from_index(index: u8) -> Option<Self> {
        (index < 81).then_some(Self(index))
    }

    pub fn from_usi(value: &str) -> Result<Self, ParseError> {
        let bytes = value.as_bytes();
        if bytes.len() != 2
            || !(b'1'..=b'9').contains(&bytes[0])
            || !(b'a'..=b'i').contains(&bytes[1])
        {
            return Err(error("Square must be 1a through 9i"));
        }
        Ok(Self((bytes[0] - b'1') * 9 + bytes[1] - b'a'))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn to_usi(self) -> String {
        format!("{}{}", self.0 / 9 + 1, (b'a' + self.0 % 9) as char)
    }
}

/// Two 64-bit words; only the bottom 17 bits of hi are meaningful.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bitboard {
    lo: u64,
    hi: u64,
}

impl Bitboard {
    pub const EMPTY: Self = Self { lo: 0, hi: 0 };

    pub fn from_words(lo: u64, hi: u64) -> Option<Self> {
        (hi & !HIGH_MASK == 0).then_some(Self { lo, hi })
    }

    pub fn words(self) -> [u64; 2] {
        [self.lo, self.hi]
    }

    pub fn insert(&mut self, square: Square) {
        if square.0 < 64 {
            self.lo |= 1u64 << square.0;
        } else {
            self.hi |= 1u64 << (square.0 - 64);
        }
    }

    pub fn contains(self, square: Square) -> bool {
        if square.0 < 64 {
            self.lo & (1u64 << square.0) != 0
        } else {
            self.hi & (1u64 << (square.0 - 64)) != 0
        }
    }

    pub fn count(self) -> u32 {
        self.lo.count_ones() + self.hi.count_ones()
    }

    pub fn squares(self) -> impl Iterator<Item = Square> {
        let mut board = self;
        std::iter::from_fn(move || {
            if board.lo != 0 {
                let bit = board.lo.trailing_zeros() as u8;
                board.lo &= board.lo - 1;
                Some(Square(bit))
            } else if board.hi != 0 {
                let bit = board.hi.trailing_zeros() as u8;
                board.hi &= board.hi - 1;
                Some(Square(bit + 64))
            } else {
                None
            }
        })
    }
}

impl BitOr for Bitboard {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self {
            lo: self.lo | rhs.lo,
            hi: self.hi | rhs.hi,
        }
    }
}

impl BitAnd for Bitboard {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self {
            lo: self.lo & rhs.lo,
            hi: self.hi & rhs.hi,
        }
    }
}

impl Not for Bitboard {
    type Output = Self;
    fn not(self) -> Self {
        Self {
            lo: !self.lo,
            hi: !self.hi & HIGH_MASK,
        }
    }
}

/// CPU array-of-structures layout. GPU batching may transpose this into SoA.
/// All fields have explicit sizes; no pointers, allocation, or native enums.
#[repr(C)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub pieces: [[Bitboard; 15]; 2],
    pub occupied: [Bitboard; 2],
    pub move_number: u32,
    pub hands: [[u8; 7]; 2],
    pub side_to_move: u8,
    pub reserved: [u8; 5],
}

impl Position {
    /// Structural validation; king safety and reachability require the verifier.
    pub fn from_sfen(sfen: &str) -> Result<Self, ParseError> {
        if !sfen.is_ascii() {
            return Err(error("SFEN must be ASCII"));
        }
        let fields: Vec<_> = sfen.split_whitespace().collect();
        if fields.len() != 4 {
            return Err(error("SFEN needs four fields"));
        }
        let side_to_move = match fields[1] {
            "b" => 0,
            "w" => 1,
            _ => return Err(error("Invalid side to move")),
        };
        let move_number = fields[3]
            .parse::<u32>()
            .map_err(|_| error("Invalid move number"))?;
        if move_number == 0
            || move_number >= 1 << 31
            || !fields[3].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(error("Move number must be positive and fit int32"));
        }
        let mut result = Self {
            pieces: [[Bitboard::EMPTY; 15]; 2],
            occupied: [Bitboard::EMPTY; 2],
            hands: [[0; 7]; 2],
            side_to_move,
            reserved: [0; 5],
            move_number,
        };
        let ranks: Vec<_> = fields[0].split('/').collect();
        if ranks.len() != 9 {
            return Err(error("SFEN needs nine ranks"));
        }
        let mut material = [0u16; 8];
        for (rank, row) in ranks.iter().enumerate() {
            let mut file = 0usize;
            let mut promoted = false;
            for piece in row.bytes() {
                if piece == b'+' && !promoted {
                    promoted = true;
                    continue;
                }
                if (b'1'..=b'9').contains(&piece) && !promoted {
                    file += (piece - b'0') as usize;
                } else {
                    let base = PIECES
                        .iter()
                        .position(|&p| p == piece.to_ascii_uppercase())
                        .ok_or_else(|| error("Invalid board piece"))?
                        + 1;
                    if file >= 9 || (promoted && (base == 7 || base == 8)) {
                        return Err(error("Invalid square or promotion"));
                    }
                    let color = usize::from(piece.is_ascii_lowercase());
                    let kind = base + if promoted { 8 } else { 0 };
                    let square = Square(((8 - file) * 9 + rank) as u8);
                    result.pieces[color][kind].insert(square);
                    result.occupied[color].insert(square);
                    material[base - 1] += 1;
                    file += 1;
                }
                promoted = false;
            }
            if file != 9 || promoted {
                return Err(error("Rank width must equal nine"));
            }
        }
        if result.pieces[0][8].count() != 1 || result.pieces[1][8].count() != 1 {
            return Err(error("One king of each color is required"));
        }
        if fields[2] != "-" {
            let mut count = 0u16;
            for piece in fields[2].bytes() {
                if piece.is_ascii_digit() {
                    if count == 0 && piece == b'0' {
                        return Err(error("Invalid hand count"));
                    }
                    count = count
                        .checked_mul(10)
                        .and_then(|n| n.checked_add((piece - b'0') as u16))
                        .filter(|&n| n <= 18)
                        .ok_or_else(|| error("Hand count exceeds limit"))?;
                    continue;
                }
                let base = PIECES[..7]
                    .iter()
                    .position(|&p| p == piece.to_ascii_uppercase())
                    .ok_or_else(|| error("Invalid hand piece"))?
                    + 1;
                let hand = HAND_TYPES.iter().position(|&p| p as usize == base).unwrap();
                let color = usize::from(piece.is_ascii_lowercase());
                if result.hands[color][hand] != 0 {
                    return Err(error("Duplicate hand piece"));
                }
                let number = count.max(1);
                result.hands[color][hand] = number as u8;
                material[base - 1] += number;
                count = 0;
            }
            if count != 0 {
                return Err(error("Hand count has no piece"));
            }
        }
        if material
            .iter()
            .zip(MAX_MATERIAL)
            .any(|(&n, maximum)| n > maximum)
        {
            return Err(error("Material exceeds standard shogi inventory"));
        }
        Ok(result)
    }

    pub fn piece_at(&self, square: Square) -> u8 {
        for color in 0..2 {
            if self.occupied[color].contains(square) {
                for kind in 1..15 {
                    if self.pieces[color][kind].contains(square) {
                        return kind as u8 | ((color as u8) << 4);
                    }
                }
            }
        }
        0
    }

    pub fn to_sfen(&self) -> String {
        let mut board = String::new();
        for rank in 0..9 {
            if rank > 0 {
                board.push('/');
            }
            let mut empty = 0u8;
            for file in (0..9).rev() {
                let piece = self.piece_at(Square(file * 9 + rank));
                if piece == 0 {
                    empty += 1;
                    continue;
                }
                if empty > 0 {
                    board.push((b'0' + empty) as char);
                    empty = 0;
                }
                let mut kind = piece & 15;
                if kind > 8 {
                    board.push('+');
                    kind -= 8;
                }
                let symbol = PIECES[(kind - 1) as usize];
                board.push(if piece & 16 != 0 {
                    symbol.to_ascii_lowercase()
                } else {
                    symbol
                } as char);
            }
            if empty > 0 {
                board.push((b'0' + empty) as char);
            }
        }
        let mut hand = String::new();
        for color in 0..2 {
            for index in [6, 5, 4, 3, 2, 1, 0] {
                let count = self.hands[color][index];
                if count == 0 {
                    continue;
                }
                if count > 1 {
                    hand.push_str(&count.to_string());
                }
                let symbol = PIECES[(HAND_TYPES[index] - 1) as usize];
                hand.push(if color == 1 {
                    symbol.to_ascii_lowercase()
                } else {
                    symbol
                } as char);
            }
        }
        if hand.is_empty() {
            hand.push('-');
        }
        format!(
            "{} {} {} {}",
            board,
            if self.side_to_move == 0 { "b" } else { "w" },
            hand,
            self.move_number
        )
    }
}

/// Lower 16 bits of cshogi moves: to[0:7], from/drop[7:14], promotion[14].
/// Syntactic encoding only: legality depends on a Position and a move generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move16(u16);

impl Move16 {
    pub fn destination(self) -> Square {
        Square((self.0 & 127) as u8)
    }

    pub fn source(self) -> Option<Square> {
        let source = ((self.0 >> 7) & 127) as u8;
        (source < 81).then_some(Square(source))
    }

    pub fn drop_kind(self) -> Option<u8> {
        let source = ((self.0 >> 7) & 127) as u8;
        (source >= 81).then(|| source - 80)
    }

    pub fn promotes(self) -> bool {
        self.0 & (1 << 14) != 0
    }

    pub fn from_usi(value: &str) -> Result<Self, ParseError> {
        if !value.is_ascii() {
            return Err(error("Move must be ASCII"));
        }
        let bytes = value.as_bytes();
        if bytes.len() == 4 && bytes[1] == b'*' {
            let kind = PIECES[..7]
                .iter()
                .position(|&p| p == bytes[0])
                .ok_or_else(|| error("Invalid drop piece"))?
                + 1;
            let to = Square::from_usi(&value[2..4])?;
            return Ok(Self(((kind as u16 + 80) << 7) | to.0 as u16));
        }
        if bytes.len() != 4 && !(bytes.len() == 5 && bytes[4] == b'+') {
            return Err(error("Invalid move syntax"));
        }
        let from = Square::from_usi(&value[..2])?;
        let to = Square::from_usi(&value[2..4])?;
        if from == to {
            return Err(error("Source and destination are identical"));
        }
        Ok(Self(
            ((from.0 as u16) << 7) | to.0 as u16 | if bytes.len() == 5 { 1 << 14 } else { 0 },
        ))
    }

    pub fn packed(self) -> u16 {
        self.0
    }

    pub fn to_usi(self) -> String {
        let to = Square((self.0 & 127) as u8).to_usi();
        let from = ((self.0 >> 7) & 127) as u8;
        if from >= 81 {
            format!("{}*{}", PIECES[(from - 81) as usize] as char, to)
        } else {
            format!(
                "{}{}{}",
                Square(from).to_usi(),
                to,
                if self.0 & (1 << 14) != 0 { "+" } else { "" }
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitboard_all_81_squares_and_complement() {
        let mut board = Bitboard::EMPTY;
        for index in 0..81 {
            board.insert(Square::from_index(index).unwrap());
        }
        assert_eq!(board.count(), 81);
        assert_eq!(
            board.squares().map(|s| s.index()).collect::<Vec<_>>(),
            (0..81).collect::<Vec<_>>()
        );
        assert_eq!(!board, Bitboard::EMPTY);
        assert_eq!(!Bitboard::EMPTY, board);
        assert!(Bitboard::from_words(0, 1 << 17).is_none());
        assert!(Square::from_index(81).is_none());
        assert_eq!(std::mem::size_of::<Bitboard>(), 16);
        assert_eq!(std::mem::size_of::<Position>(), 536);
    }

    #[test]
    fn sfen_roundtrip_start_and_promotions_and_hands() {
        for sfen in [
            START_SFEN,
            "4k4/9/+P8/9/9/9/9/9/4K4 w RB2GSNL10Prb2gsnl7p 42",
        ] {
            let position = Position::from_sfen(sfen).unwrap();
            assert_eq!(position.to_sfen(), sfen);
            assert_eq!(Position::from_sfen(&position.to_sfen()).unwrap(), position);
        }
        let position = Position::from_sfen(START_SFEN).unwrap();
        assert_eq!(position.occupied[0].count(), 20);
        assert_eq!(position.occupied[1].count(), 20);
        assert_eq!(position.piece_at(Square::from_usi("5i").unwrap()), 8);
        assert_eq!(position.piece_at(Square::from_usi("5a").unwrap()), 24);
    }

    #[test]
    fn invalid_sfens_are_rejected() {
        for sfen in [
            "9/9 b - 1",
            "9/9/9/9/9/9/9/9/9 b - 1",
            "4k4/9/9/9/9/9/9/9/4K4 x - 1",
            "4k4/9/9/9/9/9/9/9/4K4 b 19P 1",
            "4k4/9/9/9/9/9/9/9/4K4 b PP 1",
            "4k4/9/9/9/9/9/9/9/4K4 b - 0",
            "4k4/9/9/9/9/9/9/9/4K4 b 0P 1",
            "4k4/9/9/9/9/9/9/9/4+K4 b - 1",
        ] {
            assert!(Position::from_sfen(sfen).is_err(), "{sfen}");
        }
    }

    #[test]
    fn move_roundtrip() {
        for text in ["7g7f", "2b3c+", "P*5e", "R*1a", "G*9i"] {
            assert_eq!(Move16::from_usi(text).unwrap().to_usi(), text);
        }
        for text in ["resign", "K*5e", "0a1b", "1a1a", "P*5e+", "a", "あいうえ"] {
            assert!(Move16::from_usi(text).is_err(), "{text}");
        }
    }
}
