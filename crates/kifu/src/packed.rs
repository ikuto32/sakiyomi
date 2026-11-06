//! 32-byte binary positions and 16-bit moves used by computer-shogi tools:
//! HuffmanCodedPos (`hcp`, Apery/cshogi), PackedSfen (`psfen`, YaneuraOu), and the
//! cshogi / YaneuraOu `move16` encodings.
//!
//! Bits are written LSB-first; code tables below are given in stream order (first bit
//! written first). Both formats need exactly one king per side and the full 40-piece set
//! (pieces off the board and out of hand are written as "piece box" codes).

use shogi_core::Move16;
use shogi_core::rules::Board;

use crate::error::{Error, ErrorKind, Result};
use crate::notation::{HAND_KINDS, make_usi};
use crate::record::board_from_parts;

pub const PACKED_LEN: usize = 32;
const BITS: usize = PACKED_LEN * 8;
/// Standard inventory per hand kind, in hand order P L N S G B R.
const INVENTORY: [u8; 7] = [18, 4, 4, 4, 4, 2, 2];

#[derive(Clone, Copy)]
enum Dialect {
    Hcp,
    Psfen,
}

/// Hand-order index (P L N S G B R) of an unpromoted shogi-core kind.
fn hand_slot(kind: u8) -> Option<usize> {
    HAND_KINDS.iter().position(|&k| k == kind)
}

/// Base code of a board piece (hand order P L N S G B R).
fn board_base(d: Dialect, slot: usize) -> &'static str {
    match d {
        Dialect::Hcp => ["10", "1100", "1110", "1101", "11110", "111110", "111111"][slot],
        Dialect::Psfen => ["10", "1100", "1101", "1110", "11110", "111110", "111111"][slot],
    }
}

const GOLD: usize = 4;

fn board_code(d: Dialect, slot: usize, color: u8, promoted: bool) -> String {
    let mut s = board_base(d, slot).to_owned();
    let (c, p) = (if color == 1 { '1' } else { '0' }, if promoted { '1' } else { '0' });
    match d {
        Dialect::Hcp => {
            s.push(c);
            if slot != GOLD {
                s.push(p);
            }
        }
        Dialect::Psfen => {
            if slot != GOLD {
                s.push(p);
            }
            s.push(c);
        }
    }
    s
}

fn hand_code(d: Dialect, slot: usize, color: u8) -> String {
    let c = if color == 1 { '1' } else { '0' };
    match d {
        Dialect::Hcp => format!("{}{c}", ["00", "1000", "1100", "1010", "1110", "111110", "111111"][slot]),
        Dialect::Psfen => {
            let base = &board_base(d, slot)[1..];
            if slot == GOLD { format!("{base}{c}") } else { format!("{base}0{c}") }
        }
    }
}

fn box_code(d: Dialect, slot: usize) -> String {
    match d {
        Dialect::Hcp => ["010", "10010", "11010", "10110", "10111", "1111000", "1111010"][slot].to_owned(),
        Dialect::Psfen => {
            if slot == GOLD {
                "11011".to_owned()
            } else {
                format!("{}10", &board_base(d, slot)[1..])
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Code {
    /// Bits in stream order: first bit written = bit 0.
    v: u32,
    n: u8,
}

fn code_of(s: &str) -> Code {
    let v = s.bytes().enumerate().fold(0u32, |v, (i, c)| v | u32::from(c == b'1') << i);
    Code { v, n: s.len() as u8 }
}

/// Precomputed encode tables and decode lookup tables (indexed by code length and value).
struct Tables {
    board: [[[Code; 2]; 2]; 7],
    hand: [[Code; 2]; 7],
    boxed: [Code; 7],
    /// board_lut[len][value] = 0 empty, 1 + (slot * 4 + color * 2 + promoted), or -1.
    board_lut: Vec<[i16; 256]>,
    /// hand_lut[len][value] = slot * 2 + color for hand, 100 + slot for box, or -1.
    hand_lut: Vec<[i16; 256]>,
}

fn build(d: Dialect) -> Tables {
    let mut t = Tables {
        board: [[[Code::default(); 2]; 2]; 7],
        hand: [[Code::default(); 2]; 7],
        boxed: [Code::default(); 7],
        board_lut: vec![[-1i16; 256]; 9],
        hand_lut: vec![[-1i16; 256]; 9],
    };
    t.board_lut[1][0] = 0;
    for slot in 0..7 {
        for color in 0..2u8 {
            for promoted in [false, true] {
                if promoted && slot == GOLD {
                    continue;
                }
                let c = code_of(&board_code(d, slot, color, promoted));
                t.board[slot][color as usize][usize::from(promoted)] = c;
                t.board_lut[c.n as usize][c.v as usize] = 1 + (slot * 4 + color as usize * 2 + usize::from(promoted)) as i16;
            }
            let h = code_of(&hand_code(d, slot, color));
            t.hand[slot][color as usize] = h;
            t.hand_lut[h.n as usize][h.v as usize] = (slot * 2 + color as usize) as i16;
        }
        let bx = code_of(&box_code(d, slot));
        t.boxed[slot] = bx;
        t.hand_lut[bx.n as usize][bx.v as usize] = 100 + slot as i16;
    }
    t
}

fn tables(d: Dialect) -> &'static Tables {
    static HCP: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    static PSFEN: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    match d {
        Dialect::Hcp => HCP.get_or_init(|| build(Dialect::Hcp)),
        Dialect::Psfen => PSFEN.get_or_init(|| build(Dialect::Psfen)),
    }
}

struct BitWriter {
    data: [u8; PACKED_LEN],
    pos: usize,
}

impl BitWriter {
    fn bits(&mut self, value: u32, n: usize) -> Result<()> {
        if self.pos + n > BITS {
            return Err(ErrorKind::Unsupported("position needs more than 256 bits".into()).into());
        }
        for i in 0..n {
            if value >> i & 1 == 1 {
                self.data[self.pos / 8] |= 1 << (self.pos % 8);
            }
            self.pos += 1;
        }
        Ok(())
    }
    fn code(&mut self, c: Code) -> Result<()> {
        self.bits(c.v, c.n as usize)
    }
}

struct BitReader<'a> {
    data: &'a [u8; PACKED_LEN],
    pos: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u32> {
        if self.pos >= BITS {
            return None;
        }
        let b = u32::from(self.data[self.pos / 8] >> (self.pos % 8) & 1);
        self.pos += 1;
        Some(b)
    }
    fn bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Some(v)
    }
    /// Read one prefix code using a length-indexed lookup table.
    fn code(&mut self, lut: &[[i16; 256]]) -> Option<i16> {
        let mut v = 0u32;
        for len in 1..=8 {
            v |= self.bit()? << (len - 1);
            let sym = lut[len][v as usize];
            if sym >= 0 {
                return Some(sym);
            }
        }
        None
    }
}

fn kind_of(slot: usize, promoted: bool) -> u8 {
    HAND_KINDS[slot] + if promoted { 8 } else { 0 }
}

fn encode(board: &Board, d: Dialect) -> Result<[u8; PACKED_LEN]> {
    let t = tables(d);
    let kings = |c: u8| board.squares.iter().position(|&p| p == (8 | c << 4));
    let (Some(bk), Some(wk)) = (kings(0), kings(1)) else {
        return Err(ErrorKind::Unsupported("binary positions need both kings".into()).into());
    };
    let mut w = BitWriter { data: [0; PACKED_LEN], pos: 0 };
    w.bits(u32::from(board.side == 1), 1)?;
    w.bits(bk as u32, 7)?;
    w.bits(wk as u32, 7)?;
    let mut left = INVENTORY;
    for (sq, &p) in board.squares.iter().enumerate() {
        if sq == bk || sq == wk {
            continue;
        }
        if p == 0 {
            w.bits(0, 1)?;
            continue;
        }
        let kind = p & 15;
        let base = if kind > 8 { kind - 8 } else { kind };
        let slot = hand_slot(base).ok_or_else(|| Error::new(ErrorKind::InvalidPosition("extra king".into())))?;
        w.code(t.board[slot][usize::from(p >> 4 & 1)][usize::from(kind > 8)])?;
        left[slot] = left[slot].saturating_sub(1);
    }
    for color in 0..2 {
        for slot in 0..7 {
            let n = board.hands[color][slot];
            for _ in 0..n {
                w.code(t.hand[slot][color])?;
            }
            left[slot] = left[slot].saturating_sub(n);
        }
    }
    for (slot, &n) in left.iter().enumerate() {
        for _ in 0..n {
            w.code(t.boxed[slot])?;
        }
    }
    if w.pos != BITS {
        return Err(ErrorKind::Unsupported(format!("position encodes to {} bits, not 256", w.pos)).into());
    }
    Ok(w.data)
}

fn decode(data: &[u8; PACKED_LEN], d: Dialect) -> Result<Board> {
    let corrupt = |m: &str| Error::new(ErrorKind::Corrupt(m.to_owned()));
    let t = tables(d);
    let mut r = BitReader { data, pos: 0 };
    let side = r.bit().ok_or_else(|| corrupt("short"))? as u8;
    let bk = r.bits(7).ok_or_else(|| corrupt("short"))? as usize;
    let wk = r.bits(7).ok_or_else(|| corrupt("short"))? as usize;
    if bk >= 81 || wk >= 81 || bk == wk {
        return Err(corrupt("king squares"));
    }
    let mut squares = [0u8; 81];
    squares[bk] = 8;
    squares[wk] = 8 | 16;
    for (sq, cell) in squares.iter_mut().enumerate() {
        if sq == bk || sq == wk {
            continue;
        }
        let sym = r.code(&t.board_lut).ok_or_else(|| corrupt("board code"))?;
        if sym > 0 {
            let v = (sym - 1) as usize;
            let (slot, color, promoted) = (v / 4, (v / 2 % 2) as u8, v % 2 == 1);
            *cell = kind_of(slot, promoted) | color << 4;
        }
    }
    let mut hands = [[0u8; 7]; 2];
    while r.pos < BITS {
        let sym = r.code(&t.hand_lut).ok_or_else(|| corrupt("hand code"))?;
        if sym < 100 {
            let h = &mut hands[(sym % 2) as usize][(sym / 2) as usize];
            *h = h.checked_add(1).ok_or_else(|| corrupt("hand count"))?;
        }
    }
    board_from_parts(squares, hands, side)
}

pub fn to_hcp(board: &Board) -> Result<[u8; PACKED_LEN]> {
    encode(board, Dialect::Hcp)
}
pub fn from_hcp(data: &[u8; PACKED_LEN]) -> Result<Board> {
    decode(data, Dialect::Hcp)
}
pub fn to_psfen(board: &Board) -> Result<[u8; PACKED_LEN]> {
    encode(board, Dialect::Psfen)
}
pub fn from_psfen(data: &[u8; PACKED_LEN]) -> Result<Board> {
    decode(data, Dialect::Psfen)
}

/// cshogi move16: to | from << 7 | promote << 14; drops use from = 80 + kind (P=81 … G=87).
/// Identical to shogi-core's `Move16::packed`.
pub fn to_cshogi_move16(mv: Move16) -> u16 {
    mv.packed()
}

pub fn from_cshogi_move16(v: u16) -> Option<Move16> {
    let to = (v & 127) as usize;
    let from = ((v >> 7) & 127) as usize;
    let promote = v >> 14 & 1 == 1;
    if v >> 15 != 0 {
        return None;
    }
    let usi = if from >= 81 { make_usi(None, to, false, (from - 80) as u8)? } else { make_usi(Some(from), to, promote, 0)? };
    Move16::from_usi(&usi).ok()
}

/// YaneuraOu move16: to | from << 7 (drop: kind << 7 with bit 14) | promote << 15.
pub fn to_yaneuraou_move16(mv: Move16) -> u16 {
    let to = mv.destination().index() as u16;
    match mv.source() {
        Some(s) => to | (s.index() as u16) << 7 | u16::from(mv.promotes()) << 15,
        None => to | u16::from(mv.drop_kind().unwrap_or(1)) << 7 | 1 << 14,
    }
}

pub fn from_yaneuraou_move16(v: u16) -> Option<Move16> {
    let to = (v & 127) as usize;
    let from = ((v >> 7) & 127) as usize;
    let usi = if v >> 14 & 1 == 1 {
        make_usi(None, to, false, u8::try_from(from).ok()?)?
    } else {
        make_usi(Some(from), to, v >> 15 == 1, 0)?
    };
    Move16::from_usi(&usi).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shogi_core::START_SFEN;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    // Independent test vectors (derived from the cshogi tables and YaneuraOu's packing rules).
    const VECTORS: &[(&str, &str, &str)] = &[
        (
            START_SFEN,
            "58a449210cd757217e8e4d212caf427814c2ab109e4d212cd75f213e8e49210c",
            "58a451220ceb67227e9653221caf447824c22b119e53221ceb6f223e9651220c",
        ),
        (
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 2",
            "59a449210cd757217e8e4d212caf427814c2ab109e4d112cd75f213e8e49210c",
            "59a451220ceb67227e9653221caf447824c22b119e53121ceb6f223e9651220c",
        ),
        (
            "lnsgkg1nl/1r5+B1/pppppp1pp/6p2/9/2P6/PP1PPPPPP/7R1/LNSGKGSNL w B 4",
            "59a449210cd767217e0e14617915c2a3105e85f06c8a60b9fe0ae19814c2f069",
            "59a451220ceb57227e1624e17825c223115e89f09c92e0587f13611925c2f058",
        ),
        (
            "lnsgkgsnl/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1",
            "59a449210c9742fc1c9b42585e85f0288457213c9b42582e85f8382685303c5e",
            "59a451220cab44fc2ca744385e89f0488457223ca744385689f858468930bc7e",
        ),
    ];

    #[test]
    fn known_vectors_both_ways() {
        for (sfen, hcp, psfen) in VECTORS {
            let b = Board::from_sfen(sfen).unwrap();
            assert_eq!(hex(&to_hcp(&b).unwrap()), *hcp, "hcp {sfen}");
            assert_eq!(hex(&to_psfen(&b).unwrap()), *psfen, "psfen {sfen}");
            assert_eq!(from_hcp(&unhex(hcp)).unwrap().key(), b.key());
            assert_eq!(from_psfen(&unhex(psfen)).unwrap().key(), b.key());
        }
    }

    #[test]
    fn move16_encodings() {
        let m = |s: &str| Move16::from_usi(s).unwrap();
        assert_eq!(to_cshogi_move16(m("7g7f")), 7739);
        assert_eq!(to_cshogi_move16(m("8h2b+")), 0x630A);
        assert_eq!(to_cshogi_move16(m("P*5e")), 0x28A8);
        assert_eq!(to_yaneuraou_move16(m("8h2b+")), 0xA30A);
        assert_eq!(to_yaneuraou_move16(m("P*5e")), 0x40A8);
        for s in ["7g7f", "8h2b+", "P*5e", "G*1a", "R*9i"] {
            assert_eq!(from_cshogi_move16(to_cshogi_move16(m(s))), Some(m(s)));
            assert_eq!(from_yaneuraou_move16(to_yaneuraou_move16(m(s))), Some(m(s)));
        }
        assert_eq!(from_cshogi_move16(0), None); // 1a1a
        assert_eq!(from_yaneuraou_move16(129), None); // MOVE_NULL
        assert_eq!(from_yaneuraou_move16(1 << 14 | 8 << 7), None); // king drop
    }

    #[test]
    fn random_bytes_never_panic() {
        let mut x = 0x9E3779B97F4A7C15u64;
        for _ in 0..20_000 {
            let mut d = [0u8; 32];
            for b in d.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x as u8;
            }
            let _ = from_hcp(&d);
            let _ = from_psfen(&d);
            let _ = from_cshogi_move16(x as u16);
            let _ = from_yaneuraou_move16((x >> 16) as u16);
        }
    }
}
