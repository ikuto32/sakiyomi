//! CSA standard file format (V2.2 / V3.0) reading and writing (main line only, as the
//! format has no variations). Floodgate-style evaluation comments (`'** 30 -3334FU …`) are
//! recognised by [`crate::eval_comment`].

use shogi_core::rules::Board;

use crate::error::{Error, ErrorKind, Limits, Result, check_text};
use crate::eval_comment::{self, EvalStyle};
use crate::notation::{self, file_of, hand_index, rank_of, square, unpromote};
use crate::record::{GameRecord, Special, board_from_parts};

const CODES: [&str; 15] = ["", "FU", "KY", "KE", "GI", "KA", "HI", "KI", "OU", "TO", "NY", "NK", "NG", "UM", "RY"];

pub(crate) fn code_kind(code: &str) -> Option<u8> {
    CODES.iter().position(|&c| c == code && !c.is_empty()).map(|k| k as u8)
}

pub(crate) fn kind_code(kind: u8) -> &'static str {
    CODES.get(kind as usize).copied().unwrap_or("")
}

const HEADER_MAP: &[(&str, &str)] = &[
    ("EVENT", "棋戦"),
    ("SITE", "場所"),
    ("START_TIME", "開始日時"),
    ("END_TIME", "終了日時"),
    ("TIME_LIMIT", "持ち時間"),
    ("OPENING", "戦型"),
];

const SPECIAL_MAP: &[(&str, Special)] = &[
    ("TORYO", Special::Resign),
    ("CHUDAN", Special::Interrupt),
    ("SENNICHITE", Special::Sennichite),
    ("TIME_UP", Special::TimeUp),
    ("ILLEGAL_MOVE", Special::IllegalLoss),
    ("JISHOGI", Special::Jishogi),
    ("HIKIWAKE", Special::Draw),
    ("MAX_MOVES", Special::MaxMoves),
    ("MATTA", Special::Matta),
    ("KACHI", Special::EnteringKingWin),
    ("TSUMI", Special::Checkmate),
];

struct Setup {
    squares: [u8; 81],
    hands: [[u8; 7]; 2],
    touched: bool,
}

impl Setup {
    fn hirate() -> Self {
        let b = crate::record::start_board();
        Setup { squares: b.squares, hands: b.hands, touched: true }
    }

    /// Pieces still in the box (standard 40-piece set minus what is placed).
    fn remaining(&self) -> [u8; 9] {
        let mut left = [0u8, 18, 4, 4, 4, 2, 2, 4, 2];
        for &p in &self.squares {
            if p != 0 {
                let k = unpromote(p & 15) as usize;
                left[k] = left[k].saturating_sub(1);
            }
        }
        for hand in &self.hands {
            for (i, &n) in hand.iter().enumerate() {
                let k = notation::HAND_KINDS[i] as usize;
                left[k] = left[k].saturating_sub(n);
            }
        }
        left
    }
}

/// "77" → Some(Some(square)), "00" → Some(None) (hand), anything else → None.
fn parse_sq(s: &str) -> Option<Option<usize>> {
    let b = s.as_bytes();
    if b.len() != 2 || !b[0].is_ascii_digit() || !b[1].is_ascii_digit() {
        return None;
    }
    let (f, r) = ((b[0] - b'0') as usize, (b[1] - b'0') as usize);
    if f == 0 && r == 0 {
        return Some(None);
    }
    ((1..=9).contains(&f) && (1..=9).contains(&r)).then_some(Some(square(f, r)))
}

/// Parse "+7776FU" against `board`; the statement must be ASCII.
pub(crate) fn parse_csa_move(board: &Board, s: &str) -> Result<shogi_core::Move16> {
    let bad = || Error::new(ErrorKind::UnparsableMove(s.to_owned()));
    if !s.is_ascii() || s.len() < 7 {
        return Err(bad());
    }
    let from = parse_sq(&s[1..3]).ok_or_else(bad)?;
    let to = parse_sq(&s[3..5]).flatten().ok_or_else(bad)?;
    let kind = code_kind(&s[5..7]).ok_or_else(|| Error::new(ErrorKind::InvalidPieceCode(s.to_owned())))?;
    let mv = match from {
        None => notation::try_make_move(None, to, false, kind),
        Some(f) => {
            let before = board.squares[f] & 15;
            notation::try_make_move(Some(f), to, before != kind && kind > 8, 0)
        }
    };
    mv.ok_or_else(|| Error::new(ErrorKind::IllegalMove(s.to_owned())))
}

pub fn read_csa(text: &str, limits: &Limits) -> Result<GameRecord> {
    check_text(text, limits)?;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut setup = Setup { squares: [0; 81], hands: [[0; 7]; 2], touched: false };
    let mut pending_side: Option<u8> = None;
    let mut rec: Option<GameRecord> = None;
    let mut cur = 0usize;
    let mut root_comment: Vec<String> = Vec::new();

    let finalize = |setup: &Setup, side: u8, headers: &[(String, String)], comment: &[String]| -> Result<GameRecord> {
        let board = if setup.touched {
            board_from_parts(setup.squares, setup.hands, side)?
        } else {
            let mut b = crate::record::start_board();
            b.side = side;
            b
        };
        let mut r = GameRecord::new(board);
        r.headers = headers.to_vec();
        r.nodes[0].comment = comment.join("\n");
        Ok(r)
    };

    for (no, raw) in text.lines().enumerate() {
        let at = |e: Error| e.at(no + 1);
        let line = raw.trim_end_matches(['\r', '\n']);
        if let Some(c) = line.strip_prefix('\'') {
            // '* = comment attached to the current move; '** = engine evaluation (kept verbatim,
            // recognised later); other ' lines are file comments and are dropped.
            if let Some(c) = c.strip_prefix('*') {
                match rec.as_mut() {
                    Some(r) => {
                        let node = &mut r.nodes[cur];
                        if !node.comment.is_empty() {
                            node.comment.push('\n');
                        }
                        node.comment.push_str(c);
                    }
                    None => root_comment.push(c.to_owned()),
                }
            }
            continue;
        }
        for stmt in line.split(',') {
            let s = stmt.trim();
            if s.is_empty() || s.starts_with('V') {
                continue;
            }
            if let Some(name) = s.strip_prefix("N+") {
                headers.push(("先手".into(), name.to_owned()));
                continue;
            }
            if let Some(name) = s.strip_prefix("N-") {
                headers.push(("後手".into(), name.to_owned()));
                continue;
            }
            if let Some(h) = s.strip_prefix('$') {
                if let Some((k, v)) = h.split_once(':') {
                    let key = HEADER_MAP.iter().find(|(c, _)| *c == k).map(|(_, j)| *j).unwrap_or(k);
                    headers.push((key.to_owned(), v.to_owned()));
                }
                continue;
            }
            // Everything below is positional/moves: ASCII only.
            if !s.is_ascii() {
                return Err(at(ErrorKind::UnparsableMove(s.to_owned()).into()));
            }
            if let Some(rest) = s.strip_prefix("PI") {
                setup = Setup::hirate();
                for chunk in rest.as_bytes().chunks(4) {
                    if let Some(Some(sq)) = chunk.get(..2).and_then(|c| std::str::from_utf8(c).ok()).and_then(parse_sq) {
                        setup.squares[sq] = 0;
                    }
                }
            } else if s.len() >= 2 && s.starts_with('P') && s.as_bytes()[1].is_ascii_digit() {
                let rank = (s.as_bytes()[1] - b'0') as usize;
                if !(1..=9).contains(&rank) {
                    return Err(at(ErrorKind::InvalidSquare(s.to_owned()).into()));
                }
                let cells = &s[2..];
                for f in 0..9 {
                    let cell = cells.get(f * 3..f * 3 + 3).unwrap_or(" * ");
                    let sq = square(9 - f, rank);
                    setup.squares[sq] = match cell.as_bytes()[0] {
                        b'+' | b'-' => {
                            let kind = code_kind(&cell[1..])
                                .ok_or_else(|| at(ErrorKind::InvalidPieceCode(cell.to_owned()).into()))?;
                            kind | (u8::from(cell.starts_with('-')) << 4)
                        }
                        _ => 0,
                    };
                }
                setup.touched = true;
            } else if s.starts_with("P+") || s.starts_with("P-") {
                let color = usize::from(s.as_bytes()[1] == b'-');
                for chunk in s.as_bytes()[2..].chunks(4) {
                    let Ok(c) = std::str::from_utf8(chunk) else { continue };
                    if c.len() < 4 {
                        continue;
                    }
                    let (sq, code) = (&c[..2], &c[2..]);
                    if code == "AL" {
                        let left = setup.remaining();
                        for (kind, &n) in left.iter().enumerate().skip(1).take(7) {
                            if let Some(i) = hand_index(kind as u8) {
                                setup.hands[color][i] = setup.hands[color][i].saturating_add(n);
                            }
                        }
                        continue;
                    }
                    let kind = code_kind(code).ok_or_else(|| at(ErrorKind::InvalidPieceCode(code.to_owned()).into()))?;
                    match parse_sq(sq).ok_or_else(|| at(ErrorKind::InvalidSquare(sq.to_owned()).into()))? {
                        None => {
                            let i = hand_index(kind).ok_or_else(|| at(ErrorKind::InvalidHand(code.to_owned()).into()))?;
                            setup.hands[color][i] = setup.hands[color][i].saturating_add(1);
                        }
                        Some(q) => setup.squares[q] = kind | (color as u8) << 4,
                    }
                }
                setup.touched = true;
            } else if s == "+" || s == "-" {
                let side = u8::from(s == "-");
                pending_side = Some(side);
                if rec.is_none() {
                    rec = Some(finalize(&setup, side, &headers, &root_comment).map_err(at)?);
                }
            } else if (s.starts_with('+') || s.starts_with('-')) && s.len() >= 7 {
                if rec.is_none() {
                    rec = Some(finalize(&setup, u8::from(s.starts_with('-')), &headers, &root_comment).map_err(at)?);
                }
                let Some(r) = rec.as_mut() else { continue };
                if r.nodes[cur].special.is_some() {
                    continue;
                }
                if r.nodes.len() >= limits.max_nodes {
                    return Err(at(ErrorKind::TooLarge { what: "moves", limit: limits.max_nodes }.into()));
                }
                let mv = parse_csa_move(&r.nodes[cur].board, s).map_err(at)?;
                cur = r.add_move(cur, mv).map_err(at)?;
            } else if let Some(t) = s.strip_prefix('T') {
                if let (Some(r), Ok(secs)) = (rec.as_mut(), t.trim().parse::<f64>())
                    && cur != 0 && secs.is_finite() && secs >= 0.0 {
                        r.nodes[cur].time_ms = Some((secs * 1000.0).round().min(1e15) as u64);
                    }
            } else if let Some(code) = s.strip_prefix('%') {
                let Some(r) = rec.as_mut() else { continue };
                if r.nodes[cur].special.is_some() {
                    continue;
                }
                let side_to_move = r.nodes[cur].board.side;
                let special = match code {
                    "+ILLEGAL_ACTION" | "-ILLEGAL_ACTION" => {
                        let offender = u8::from(code.starts_with('-'));
                        if offender == side_to_move { Special::IllegalLoss } else { Special::IllegalWin }
                    }
                    _ => SPECIAL_MAP.iter().find(|(c, _)| *c == code).map(|(_, j)| *j).unwrap_or(Special::Interrupt),
                };
                cur = r.add_special(cur, special);
            }
        }
    }
    let mut r = match rec {
        Some(r) => r,
        None => finalize(&setup, pending_side.unwrap_or(0), &headers, &root_comment)?,
    };
    eval_comment::extract_all(&mut r);
    Ok(r)
}

/// CSA move text, e.g. "+7776FU" (resulting piece after promotion).
pub(crate) fn csa_move(board: &Board, mv: shogi_core::Move16) -> String {
    let to = mv.destination().index();
    let (from, kind) = match mv.source() {
        Some(s) => {
            let k = board.squares[s.index()] & 15;
            (format!("{}{}", file_of(s.index()), rank_of(s.index())), if mv.promotes() { k + 8 } else { k })
        }
        None => ("00".to_owned(), mv.drop_kind().unwrap_or(1)),
    };
    format!(
        "{}{}{}{}{}",
        if board.side == 0 { '+' } else { '-' },
        from,
        file_of(to),
        rank_of(to),
        kind_code(kind)
    )
}

/// CSA position lines (PI or P1..P9 + P+/P- + side to move).
pub fn csa_position(board: &Board) -> String {
    let mut out = String::new();
    let hirate = crate::record::start_board();
    if board.squares == hirate.squares && board.hands == hirate.hands {
        out.push_str("PI\n");
    } else {
        for rank in 1..=9 {
            out.push_str(&format!("P{rank}"));
            for file in (1..=9).rev() {
                let p = board.squares[square(file, rank)];
                if p == 0 {
                    out.push_str(" * ");
                } else {
                    out.push(if p >> 4 == 1 { '-' } else { '+' });
                    out.push_str(kind_code(p & 15));
                }
            }
            out.push('\n');
        }
        for color in 0..2 {
            let mut line = String::new();
            for (i, &n) in board.hands[color].iter().enumerate() {
                for _ in 0..n {
                    line.push_str("00");
                    line.push_str(kind_code(notation::HAND_KINDS[i]));
                }
            }
            if !line.is_empty() {
                out.push_str(&format!("P{}{}\n", if color == 0 { '+' } else { '-' }, line));
            }
        }
    }
    out.push_str(if board.side == 0 { "+\n" } else { "-\n" });
    out
}

/// Parse CSA position lines into a board (no moves).
pub fn read_csa_position(text: &str, limits: &Limits) -> Result<Board> {
    let rec = read_csa(text, limits)?;
    Ok(*rec.initial())
}

pub fn write_csa(rec: &GameRecord, eval_style: Option<EvalStyle>) -> String {
    let mut out = String::from("V2.2\n");
    if let Some(n) = rec.black_name() {
        out.push_str(&format!("N+{n}\n"));
    }
    if let Some(n) = rec.white_name() {
        out.push_str(&format!("N-{n}\n"));
    }
    for (k, v) in &rec.headers {
        if let Some((code, _)) = HEADER_MAP.iter().find(|(_, j)| j == k) {
            out.push_str(&format!("${code}:{v}\n"));
        }
    }
    for c in rec.nodes[0].comment.lines() {
        out.push_str(&format!("'*{c}\n"));
    }
    out.push_str(&csa_position(rec.initial()));
    for &n in rec.mainline().iter().skip(1) {
        let node = &rec.nodes[n];
        let Some(parent) = node.parent.map(|p| &rec.nodes[p]) else { continue };
        if let Some(sp) = node.special {
            let side_to_move = parent.board.side;
            let code = match sp {
                Special::IllegalWin => if side_to_move == 0 { "-ILLEGAL_ACTION" } else { "+ILLEGAL_ACTION" },
                Special::IllegalLoss => "ILLEGAL_MOVE",
                other => SPECIAL_MAP.iter().find(|(_, j)| *j == other).map(|(c, _)| *c).unwrap_or("CHUDAN"),
            };
            out.push_str(&format!("%{code}\n"));
            break;
        }
        let Some(mv) = node.mv else { continue };
        out.push_str(&csa_move(&parent.board, mv));
        out.push('\n');
        if let Some(ms) = node.time_ms {
            out.push_str(&format!("T{}\n", ms / 1000));
        }
        for c in node.comment.lines() {
            out.push_str(&format!("'*{c}\n"));
        }
        if let Some(style) = eval_style {
            for line in eval_comment::csa_lines(rec, n, style) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
'----------棋譜ファイルの例\"example.csa\"-----------------
V2.2
N+NAKAHARA
N-YONENAGA
$EVENT:13th World Computer Shogi Championship
$START_TIME:2003/05/03 10:30:00
P1-KY-KE-GI-KI-OU-KI-GI-KE-KY
P2 * -HI *  *  *  *  * -KA *
P3-FU-FU-FU-FU-FU-FU-FU-FU-FU
P4 *  *  *  *  *  *  *  *  *
P5 *  *  *  *  *  *  *  *  *
P6 *  *  *  *  *  *  *  *  *
P7+FU+FU+FU+FU+FU+FU+FU+FU+FU
P8 * +KA *  *  *  *  * +HI *
P9+KY+KE+GI+KI+OU+KI+GI+KE+KY
+
+2726FU
T12
-3334FU
T6
+7776FU,T3
-8822UM
T1
+3122GI
'*同銀で取る
%TORYO
";

    fn lim() -> Limits {
        Limits::default()
    }

    #[test]
    fn read_standard_example() {
        // Move 4 above (-8822UM) is illegal for White and must be rejected.
        let e = read_csa(SAMPLE, &lim()).unwrap_err();
        assert_eq!(e.line, Some(22));
        let fixed = SAMPLE.replace("-8822UM\nT1\n+3122GI", "-2288UM\nT1\n+7988GI");
        let rec = read_csa(&fixed, &lim()).unwrap();
        assert_eq!(rec.black_name(), Some("NAKAHARA"));
        assert_eq!(rec.header("棋戦"), Some("13th World Computer Shogi Championship"));
        let main = rec.mainline();
        let labels: Vec<_> = main.iter().skip(1).take(5).map(|&n| rec.nodes[n].label.as_str()).collect();
        assert_eq!(labels, ["▲２六歩", "△３四歩", "▲７六歩", "△８八角成", "▲同　銀"]);
        assert_eq!(rec.nodes[main[6]].special, Some(Special::Resign));
        assert_eq!(rec.nodes[main[1]].time_ms, Some(12_000));
        assert_eq!(rec.nodes[main[3]].time_ms, Some(3_000));
        assert_eq!(rec.nodes[main[5]].comment, "同銀で取る");
    }

    #[test]
    fn csa_roundtrip_and_custom_position() {
        let rec = read_csa("PI\n+\n+7776FU\nT2\n-3334FU\n+8822UM\n-3122GI\n+0045KA\n%CHUDAN\n", &lim()).unwrap();
        assert_eq!(rec.mainline().len(), 7);
        let text = write_csa(&rec, None);
        assert_eq!(write_csa(&read_csa(&text, &lim()).unwrap(), None), text);
        assert!(text.contains("+8822UM"));
        assert!(text.contains("+0045KA"));

        let pos = "P1 *  *  *  *  * -OU *  *  * \nP2 *  *  *  *  *  *  *  *  * \nP3 *  *  *  *  *  *  *  *  * \n\
P4 *  *  *  *  *  *  *  *  * \nP5 *  *  *  *  *  *  *  *  * \nP6 *  *  *  *  *  *  *  *  * \n\
P7 *  *  *  *  *  *  *  *  * \nP8 *  *  *  *  *  *  *  *  * \nP9 *  *  *  *  * +OU *  *  * \nP+00KI\nP-00AL\n-\n";
        let rec = read_csa(pos, &lim()).unwrap();
        assert_eq!(rec.initial().side, 1);
        assert_eq!(rec.initial().hands[0][4], 1);
        assert_eq!(rec.initial().hands[1][0], 18);
        let again = read_csa(&write_csa(&rec, None), &lim()).unwrap();
        assert_eq!(again.initial().key(), rec.initial().key());
    }

    #[test]
    fn hostile_inputs_are_errors_not_panics() {
        // Dropping a king, non-ASCII move text, thousands of hand pieces.
        assert!(read_csa("PI\n+\n+0055OU\n", &lim()).is_err());
        assert!(read_csa("PI\n+\n+77あ6FU\n", &lim()).is_err());
        assert!(read_csa("PIあい\n+\n", &lim()).is_err());
        let many = format!("P+{}\n+\n", "00FU".repeat(300));
        assert!(read_csa(&many, &lim()).is_err());
        assert!(read_csa("P0 *\n", &lim()).is_err());
    }
}
