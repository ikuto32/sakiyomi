//! Japanese move notation (KIF / KI2 style) on top of shogi-core boards.
//!
//! Square index = (file - 1) * 9 + (rank - 1), matching `shogi_core::Square`.
//! Piece kinds: 1 歩 2 香 3 桂 4 銀 5 角 6 飛 7 金 8 玉, +8 for promoted pieces.

use crate::error::{Error, ErrorKind};
use shogi_core::Move16;
use shogi_core::rules::Board;

pub const FW_DIGITS: [char; 10] = ['０', '１', '２', '３', '４', '５', '６', '７', '８', '９'];
pub const KANJI_DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
/// USI letters indexed by kind - 1 (unpromoted kinds only).
pub const USI_LETTERS: [char; 8] = ['P', 'L', 'N', 'S', 'B', 'R', 'G', 'K'];
/// Hand index order used by shogi-core: 歩 香 桂 銀 金 角 飛.
pub const HAND_KINDS: [u8; 7] = [1, 2, 3, 4, 7, 5, 6];

pub fn file_of(sq: usize) -> usize {
    sq / 9 + 1
}
pub fn rank_of(sq: usize) -> usize {
    sq % 9 + 1
}
pub fn square(file: usize, rank: usize) -> usize {
    (file - 1) * 9 + rank - 1
}
pub fn hand_index(kind: u8) -> Option<usize> {
    HAND_KINDS.iter().position(|&k| k == kind)
}
pub fn unpromote(kind: u8) -> u8 {
    if kind > 8 { kind - 8 } else { kind }
}

pub fn piece_name(kind: u8) -> &'static str {
    match kind {
        1 => "歩",
        2 => "香",
        3 => "桂",
        4 => "銀",
        5 => "角",
        6 => "飛",
        7 => "金",
        8 => "玉",
        9 => "と",
        10 => "成香",
        11 => "成桂",
        12 => "成銀",
        13 => "馬",
        14 => "龍",
        _ => "？",
    }
}

/// Single-character name used in BOD board diagrams.
pub fn piece_char(kind: u8) -> char {
    match kind {
        10 => '杏',
        11 => '圭',
        12 => '全',
        _ => piece_name(kind).chars().next().unwrap_or('？'),
    }
}

/// Parse a piece name at the start of `s`; returns (kind, bytes consumed).
pub fn parse_piece_prefix(s: &str) -> Option<(u8, usize)> {
    for (name, kind) in [("成香", 10), ("成桂", 11), ("成銀", 12), ("成角", 13), ("成飛", 14)] {
        if s.starts_with(name) {
            return Some((kind, name.len()));
        }
    }
    let c = s.chars().next()?;
    let kind = match c {
        '歩' => 1,
        '香' => 2,
        '桂' => 3,
        '銀' => 4,
        '角' => 5,
        '飛' => 6,
        '金' => 7,
        '玉' | '王' => 8,
        'と' => 9,
        '杏' => 10,
        '圭' => 11,
        '全' => 12,
        '馬' => 13,
        '龍' | '竜' => 14,
        _ => return None,
    };
    Some((kind, c.len_utf8()))
}

pub fn parse_digit_char(c: char) -> Option<usize> {
    if let Some(d) = c.to_digit(10) {
        return (d > 0).then_some(d as usize);
    }
    if let Some(i) = FW_DIGITS.iter().position(|&d| d == c) {
        return (i > 0).then_some(i);
    }
    KANJI_DIGITS.iter().position(|&d| d == c).filter(|&i| i > 0)
}

/// 1..=18 as kanji (一 .. 十八), used for hand counts.
pub fn kanji_number(n: u32) -> String {
    match n {
        0 => String::new(),
        1..=9 => KANJI_DIGITS[n as usize].to_string(),
        10 => "十".to_owned(),
        _ => format!("十{}", KANJI_DIGITS[(n % 10) as usize]),
    }
}

pub fn parse_kanji_number(s: &str) -> Option<u32> {
    if s.is_empty() {
        return Some(1);
    }
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    let mut total = 0;
    let mut chars = s.chars().peekable();
    if chars.peek() == Some(&'十') {
        chars.next();
        total = 10;
    }
    for c in chars {
        total += KANJI_DIGITS.iter().position(|&d| d == c)? as u32;
    }
    Some(total)
}

pub fn square_jp(sq: usize) -> String {
    format!("{}{}", FW_DIGITS[file_of(sq)], KANJI_DIGITS[rank_of(sq)])
}

pub fn in_zone(side: u8, sq: usize) -> bool {
    let rank = sq % 9;
    if side == 0 { rank < 3 } else { rank >= 6 }
}

/// USI text for a move; `None` if the parts cannot form a move (e.g. dropping a king).
pub fn make_usi(from: Option<usize>, to: usize, promote: bool, drop_kind: u8) -> Option<String> {
    if to >= 81 || from.is_some_and(|f| f >= 81) {
        return None;
    }
    let to_s = sq_usi(to);
    match from {
        Some(f) => Some(format!("{}{}{}", sq_usi(f), to_s, if promote { "+" } else { "" })),
        None if (1..=7).contains(&drop_kind) => Some(format!("{}*{}", USI_LETTERS[(drop_kind - 1) as usize], to_s)),
        None => None,
    }
}

pub fn sq_usi(sq: usize) -> String {
    format!("{}{}", file_of(sq), (b'a' + (sq % 9) as u8) as char)
}

/// Build a move from parts; `None` for impossible encodings (never panics on file data).
pub fn try_make_move(from: Option<usize>, to: usize, promote: bool, drop_kind: u8) -> Option<Move16> {
    Move16::from_usi(&make_usi(from, to, promote, drop_kind)?).ok()
}

pub struct MoveFacts {
    pub side: u8,
    pub from: Option<usize>,
    pub to: usize,
    /// Moving piece kind before the move (drop kind for drops).
    pub kind: u8,
    pub promote: bool,
    pub can_promote: bool,
    pub captured: u8,
}

pub fn facts(board: &Board, mv: Move16) -> MoveFacts {
    let side = board.side;
    let to = mv.destination().index();
    let captured = board.squares[to] & 15;
    match mv.source() {
        Some(src) => {
            let from = src.index();
            let kind = board.squares[from] & 15;
            MoveFacts {
                side,
                from: Some(from),
                to,
                kind,
                promote: mv.promotes(),
                can_promote: (1..=6).contains(&kind) && (in_zone(side, from) || in_zone(side, to)),
                captured,
            }
        }
        None => MoveFacts {
            side,
            from: None,
            to,
            kind: mv.drop_kind().unwrap_or(1),
            promote: false,
            can_promote: false,
            captured,
        },
    }
}

fn dest_text(to: usize, prev_to: Option<usize>) -> String {
    if prev_to == Some(to) { "同　".to_owned() } else { square_jp(to) }
}

/// KIF style, e.g. `７六歩(77)`, `同　角成(88)`, `５五角打`.
pub fn kif_move(board: &Board, mv: Move16, prev_to: Option<usize>) -> String {
    let f = facts(board, mv);
    let mut s = dest_text(f.to, prev_to);
    s.push_str(piece_name(f.kind));
    match f.from {
        Some(from) => {
            if f.promote {
                s.push('成');
            } else if f.can_promote {
                s.push_str("不成");
            }
            s.push_str(&format!("({}{})", file_of(from), rank_of(from)));
        }
        None => s.push('打'),
    }
    s
}

/// 1 = forward (上), 0 = sideways (寄), -1 = backward (引), from the mover's view.
fn direction(side: u8, from: usize, to: usize) -> i32 {
    let dy = rank_of(to) as i32 - rank_of(from) as i32;
    let forward = if side == 0 { -dy } else { dy };
    forward.signum()
}

/// Larger is further to the mover's right.
fn right_key(side: u8, sq: usize) -> i32 {
    let file = file_of(sq) as i32;
    if side == 0 { -file } else { file }
}

fn dir_text(d: i32) -> &'static str {
    match d {
        1 => "上",
        0 => "寄",
        _ => "引",
    }
}

fn gold_like(kind: u8) -> bool {
    matches!(kind, 4 | 7 | 9 | 10 | 11 | 12)
}

/// Board sources (other than drops) of `kind` that can legally reach `to`.
fn sources(board: &Board, legal: &[Move16], to: usize, kind: u8) -> Vec<usize> {
    let mut v: Vec<usize> = legal
        .iter()
        .filter(|m| m.destination().index() == to)
        .filter_map(|m| m.source().map(|s| s.index()))
        .filter(|&s| board.squares[s] & 15 == kind)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

pub(crate) fn disambiguation(board: &Board, f: &MoveFacts, legal: &[Move16]) -> String {
    let cands = sources(board, legal, f.to, f.kind);
    let Some(from) = f.from else {
        return if cands.is_empty() { String::new() } else { "打".to_owned() };
    };
    let others: Vec<usize> = cands.into_iter().filter(|&s| s != from).collect();
    if others.is_empty() {
        return String::new();
    }
    let side = f.side;
    let my_dir = direction(side, from, f.to);
    if gold_like(f.kind) && file_of(from) == file_of(f.to) && my_dir == 1 {
        return "直".to_owned();
    }
    if others.iter().all(|&s| direction(side, s, f.to) != my_dir) {
        return dir_text(my_dir).to_owned();
    }
    let me = right_key(side, from);
    if others.iter().all(|&s| right_key(side, s) < me) {
        return "右".to_owned();
    }
    if others.iter().all(|&s| right_key(side, s) > me) {
        return "左".to_owned();
    }
    let same: Vec<usize> = others
        .iter()
        .copied()
        .filter(|&s| direction(side, s, f.to) == my_dir)
        .collect();
    let lr = if same.iter().all(|&s| right_key(side, s) < me) {
        "右"
    } else if same.iter().all(|&s| right_key(side, s) > me) {
        "左"
    } else {
        ""
    };
    format!("{}{}", lr, dir_text(my_dir))
}

/// KI2 style with side mark, e.g. `▲７六歩`, `△同　銀左`, `▲５八金右`.
pub fn ki2_move(board: &Board, mv: Move16, prev_to: Option<usize>) -> String {
    let legal = board.legal_moves();
    ki2_move_with(board, mv, prev_to, &legal)
}

pub fn ki2_move_with(board: &Board, mv: Move16, prev_to: Option<usize>, legal: &[Move16]) -> String {
    let f = facts(board, mv);
    let mut s = String::from(if f.side == 0 { "▲" } else { "△" });
    s.push_str(&dest_text(f.to, prev_to));
    s.push_str(piece_name(f.kind));
    s.push_str(&disambiguation(board, &f, legal));
    if f.promote {
        s.push('成');
    } else if f.can_promote {
        s.push_str("不成");
    }
    s
}

/// Parse a KIF or KI2 move (with or without `▲`/`△`, with or without `(77)` source).
pub fn parse_jp_move(board: &Board, text: &str, prev_to: Option<usize>) -> Result<Move16, Error> {
    let original = text;
    let mut s = text.trim_start_matches(|c: char| matches!(c, '▲' | '△' | '☗' | '☖') || c.is_whitespace());
    let to = if let Some(rest) = s.strip_prefix('同') {
        s = rest.trim_start_matches(char::is_whitespace);
        prev_to.ok_or_else(|| Error::new(ErrorKind::SameWithoutPrevious(original.to_owned())))?
    } else {
        let mut chars = s.chars();
        let (Some(fc), Some(rc)) = (chars.next(), chars.next()) else {
            return Err(ErrorKind::UnparsableMove(original.to_owned()).into());
        };
        let (Some(file), Some(rank)) = (parse_digit_char(fc), parse_digit_char(rc)) else {
            return Err(ErrorKind::UnparsableMove(original.to_owned()).into());
        };
        s = &s[fc.len_utf8() + rc.len_utf8()..];
        square(file, rank)
    };
    let (kind, used) =
        parse_piece_prefix(s).ok_or_else(|| Error::new(ErrorKind::UnparsableMove(original.to_owned())))?;
    let mut rest = &s[used..];
    let mut source = None;
    if let Some(p) = rest.find('(') {
        let inner: Vec<usize> = rest[p + 1..]
            .chars()
            .take_while(|&c| c != ')')
            .filter_map(parse_digit_char)
            .collect();
        if inner.len() == 2 {
            source = Some(square(inner[0], inner[1]));
        }
        rest = &rest[..p];
    }
    let no_promote = rest.contains("不成") || rest.contains('生');
    let promote = !no_promote && rest.contains('成');
    let drop = rest.contains('打');

    let legal = board.legal_moves();
    let at_to: Vec<Move16> = legal
        .iter()
        .copied()
        .filter(|m| m.destination().index() == to)
        .collect();
    let drops: Vec<Move16> = at_to.iter().copied().filter(|m| m.drop_kind() == Some(kind)).collect();
    if drop {
        return drops.first().copied().ok_or_else(|| ErrorKind::IllegalMove(original.to_owned()).into());
    }
    let mut cands: Vec<Move16> = at_to
        .iter()
        .copied()
        .filter(|m| m.promotes() == promote)
        .filter(|m| {
            m.source()
                .is_some_and(|src| board.squares[src.index()] & 15 == kind && source.is_none_or(|s| s == src.index()))
        })
        .collect();
    if source.is_none() && cands.len() > 1 {
        let side = board.side;
        // Every candidate is a board move (filtered above), so the fallback is never used.
        let from_of = |m: &Move16| m.source().map_or(to, |s| s.index());
        if rest.contains('直') {
            cands.retain(|m| file_of(from_of(m)) == file_of(to) && direction(side, from_of(m), to) == 1);
        }
        if rest.contains('上') || rest.contains('行') {
            cands.retain(|m| direction(side, from_of(m), to) == 1);
        }
        if rest.contains('引') {
            cands.retain(|m| direction(side, from_of(m), to) == -1);
        }
        if rest.contains('寄') {
            cands.retain(|m| direction(side, from_of(m), to) == 0);
        }
        if rest.contains('右')
            && let Some(best) = cands.iter().map(|m| right_key(side, from_of(m))).max() {
                cands.retain(|m| right_key(side, from_of(m)) == best);
            }
        if rest.contains('左')
            && let Some(best) = cands.iter().map(|m| right_key(side, from_of(m))).min() {
                cands.retain(|m| right_key(side, from_of(m)) == best);
            }
    }
    match cands.len() {
        1 => Ok(cands[0]),
        0 if source.is_none() && !drops.is_empty() => Ok(drops[0]),
        0 => Err(ErrorKind::IllegalMove(original.to_owned()).into()),
        _ => Err(ErrorKind::AmbiguousMove(original.to_owned()).into()),
    }
}

/// Western (Hosking-style) letters indexed by piece kind.
const WESTERN: [&str; 15] = ["", "P", "L", "N", "S", "B", "R", "G", "K", "+P", "+L", "+N", "+S", "+B", "+R"];

/// Western notation with side mark, e.g. `☗P-7f`, `☖Bx8h+`, `☗G6i-5h`, `☗S*5e`, `☗N-3c=`.
/// The origin square is given only when another piece of the same kind could also move there;
/// `=` marks a declined promotion.
pub fn western_move_with(board: &Board, mv: Move16, legal: &[Move16]) -> String {
    let f = facts(board, mv);
    let mut s = String::from(if f.side == 0 { "☗" } else { "☖" });
    s.push_str(WESTERN[f.kind as usize]);
    match f.from {
        None => {
            s.push('*');
            s.push_str(&sq_usi(f.to));
        }
        Some(from) => {
            if sources(board, legal, f.to, f.kind).iter().any(|&o| o != from) {
                s.push_str(&sq_usi(from));
            }
            s.push(if f.captured != 0 { 'x' } else { '-' });
            s.push_str(&sq_usi(f.to));
            if f.promote {
                s.push('+');
            } else if f.can_promote {
                s.push('=');
            }
        }
    }
    s
}

/// A USI principal variation in Western notation, space separated.
pub fn pv_to_western(board: &Board, pv: &[String]) -> String {
    let mut b = *board;
    let mut out = Vec::new();
    for text in pv {
        let Ok(mv) = Move16::from_usi(text) else { break };
        let legal = b.legal_moves();
        if !legal.contains(&mv) {
            break;
        }
        out.push(western_move_with(&b, mv, &legal));
        b = b.after_generated(mv);
    }
    out.join(" ")
}

/// Convert a USI principal variation to KI2 text, stopping at the first illegal move.
pub fn pv_to_jp(board: &Board, pv: &[String], prev_to: Option<usize>) -> String {
    let mut b = *board;
    let mut prev = prev_to;
    let mut out = String::new();
    for text in pv {
        let Ok(mv) = Move16::from_usi(text) else { break };
        let legal = b.legal_moves();
        if !legal.contains(&mv) {
            break;
        }
        out.push_str(&ki2_move_with(&b, mv, prev, &legal));
        prev = Some(mv.destination().index());
        b = b.after_generated(mv);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shogi_core::START_SFEN;

    fn mv(s: &str) -> Move16 {
        Move16::from_usi(s).unwrap()
    }

    #[test]
    fn kif_and_ki2_basic() {
        let b = Board::from_sfen(START_SFEN).unwrap();
        assert_eq!(kif_move(&b, mv("7g7f"), None), "７六歩(77)");
        assert_eq!(ki2_move(&b, mv("7g7f"), None), "▲７六歩");
        // Two golds can reach 5h: 6i and 4i.
        assert_eq!(ki2_move(&b, mv("4i5h"), None), "▲５八金右");
        assert_eq!(ki2_move(&b, mv("6i5h"), None), "▲５八金左");
        assert_eq!(parse_jp_move(&b, "▲５八金右", None).unwrap(), mv("4i5h"));
        assert_eq!(parse_jp_move(&b, "５八金(69)", None).unwrap(), mv("6i5h"));
        assert_eq!(parse_jp_move(&b, "7六歩", None).unwrap(), mv("7g7f"));
    }

    #[test]
    fn gold_straight_and_drop_marker() {
        // Golds on 5i and 4i, both reach 5h: 5i->5h is 直, 4i->5h is 右.
        let b = Board::from_sfen("4k4/9/9/9/9/9/9/9/3KGG3 b G 1").unwrap();
        let straight = ki2_move(&b, mv("5i5h"), None);
        assert_eq!(straight, "▲５八金直");
        assert_eq!(parse_jp_move(&b, &straight, None).unwrap(), mv("5i5h"));
        assert_eq!(ki2_move(&b, mv("4i5h"), None), "▲５八金右");
        assert_eq!(ki2_move(&b, mv("G*5h"), None), "▲５八金打");
        assert_eq!(parse_jp_move(&b, "▲５八金打", None).unwrap(), mv("G*5h"));
        // Drop with no board candidate: KI2 omits 打, KIF keeps it.
        assert_eq!(ki2_move(&b, mv("G*5e"), None), "▲５五金");
        assert_eq!(kif_move(&b, mv("G*5e"), None), "５五金打");
        assert_eq!(parse_jp_move(&b, "▲５五金", None).unwrap(), mv("G*5e"));
    }

    #[test]
    fn promotion_and_same_square() {
        let b = Board::from_sfen("lnsgkgsnl/1r5b1/pppppp1pp/6p2/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL b - 3").unwrap();
        assert_eq!(kif_move(&b, mv("8h2b+"), None), "２二角成(88)");
        assert_eq!(kif_move(&b, mv("8h2b"), None), "２二角不成(88)");
        let after = b.play(mv("8h2b+")).unwrap();
        assert_eq!(kif_move(&after, mv("3a2b"), Some(mv("8h2b+").destination().index())), "同　銀(31)");
        assert_eq!(parse_jp_move(&after, "同　銀(31)", Some(square(2, 2))).unwrap(), mv("3a2b"));
        assert_eq!(parse_jp_move(&b, "２二角成(88)", None).unwrap(), mv("8h2b+"));
        assert_eq!(parse_jp_move(&b, "▲２二角不成", None).unwrap(), mv("8h2b"));
    }

    #[test]
    fn kanji_numbers_and_pv() {
        assert_eq!(kanji_number(18), "十八");
        assert_eq!(parse_kanji_number("十八"), Some(18));
        assert_eq!(parse_kanji_number("三"), Some(3));
        let b = Board::from_sfen(START_SFEN).unwrap();
        let pv = ["7g7f", "3c3d", "8h2b+", "3a2b"].map(String::from);
        assert_eq!(pv_to_jp(&b, &pv, None), "▲７六歩△３四歩▲２二角成△同　銀");
        assert_eq!(pv_to_western(&b, &pv), "☗P-7f ☖P-3d ☗Bx2b+ ☖Sx2b");
    }

    #[test]
    fn western_disambiguation_and_markers() {
        let b = Board::from_sfen(START_SFEN).unwrap();
        let legal = b.legal_moves();
        assert_eq!(western_move_with(&b, mv("4i5h"), &legal), "☗G4i-5h");
        assert_eq!(western_move_with(&b, mv("2h3h"), &legal), "☗R-3h");
        let b = Board::from_sfen("4k4/9/9/9/9/9/9/9/3KGG3 b G 1").unwrap();
        let legal = b.legal_moves();
        assert_eq!(western_move_with(&b, mv("G*5e"), &legal), "☗G*5e");
        let b = Board::from_sfen("lnsgkgsnl/1r5b1/pppppp1pp/6p2/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL b - 3").unwrap();
        let legal = b.legal_moves();
        assert_eq!(western_move_with(&b, mv("8h2b"), &legal), "☗Bx2b=");
    }
}
