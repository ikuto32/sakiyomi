//! JSON Kifu Format (JKF 1.0, <https://github.com/na2hiro/json-kifu-format>).
//!
//! `moves[0]` carries only comments; each later element is one move or special. `forks`
//! on element *i* are alternative lines whose first move replaces the move at *i*.

use serde_json::{Map, Value, json};
use shogi_core::rules::Board;

use crate::eval_comment::EvalStyle;
use crate::error::{Error, ErrorKind, Limits, Result, check_text};
use crate::notation::{self, file_of, rank_of, square};
use crate::record::{GameRecord, Special, board_from_parts, handicap_name, handicap_sfen, parse_sfen};

const KINDS: [&str; 15] = ["", "FU", "KY", "KE", "GI", "KA", "HI", "KI", "OU", "TO", "NY", "NK", "NG", "UM", "RY"];
const PRESETS: &[(&str, &str)] = &[
    ("HIRATE", "平手"),
    ("KY", "香落ち"),
    ("KY_R", "右香落ち"),
    ("KA", "角落ち"),
    ("HI", "飛車落ち"),
    ("HIKY", "飛香落ち"),
    ("2", "二枚落ち"),
    ("3", "三枚落ち"),
    ("4", "四枚落ち"),
    ("5", "五枚落ち"),
    ("5_L", "左五枚落ち"),
    ("6", "六枚落ち"),
    ("7_L", "左七枚落ち"),
    ("7_R", "右七枚落ち"),
    ("8", "八枚落ち"),
    ("10", "十枚落ち"),
];
const SPECIALS: &[(&str, Special)] = &[
    ("TORYO", Special::Resign),
    ("CHUDAN", Special::Interrupt),
    ("SENNICHITE", Special::Sennichite),
    ("TIME_UP", Special::TimeUp),
    ("ILLEGAL_MOVE", Special::IllegalLoss),
    ("JISHOGI", Special::Jishogi),
    ("KACHI", Special::EnteringKingWin),
    ("HIKIWAKE", Special::Draw),
    ("MAX_MOVES", Special::MaxMoves),
    ("MATTA", Special::Matta),
    ("TSUMI", Special::Checkmate),
];
/// Variation nesting accepted when writing (JSON nesting follows it).
const MAX_FORK_DEPTH: usize = 100;

fn kind_index(s: &str) -> Option<u8> {
    KINDS.iter().position(|&k| k == s && !k.is_empty()).map(|i| i as u8)
}

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::Corrupt(msg.into()))
}

fn sq_of(v: &Value) -> Option<usize> {
    let x = v.get("x")?.as_u64()?;
    let y = v.get("y")?.as_u64()?;
    ((1..=9).contains(&x) && (1..=9).contains(&y)).then(|| square(x as usize, y as usize))
}

fn initial_board(v: Option<&Value>) -> Result<Board> {
    let Some(init) = v.filter(|v| !v.is_null()) else { return parse_sfen(shogi_core::START_SFEN) };
    let preset = init.get("preset").and_then(Value::as_str).unwrap_or("HIRATE");
    if preset != "OTHER" {
        let name = PRESETS.iter().find(|(p, _)| *p == preset).map(|(_, n)| *n).ok_or_else(|| bad(format!("preset {preset}")))?;
        return parse_sfen(handicap_sfen(name).ok_or_else(|| bad("preset"))?);
    }
    let data = init.get("data").ok_or_else(|| bad("initial.data"))?;
    let side = data.get("color").and_then(Value::as_u64).unwrap_or(0).min(1) as u8;
    let mut squares = [0u8; 81];
    let board = data.get("board").and_then(Value::as_array).ok_or_else(|| bad("initial.data.board"))?;
    for (x, col) in board.iter().take(9).enumerate() {
        for (y, cell) in col.as_array().map(Vec::as_slice).unwrap_or(&[]).iter().take(9).enumerate() {
            let (Some(kind), Some(color)) =
                (cell.get("kind").and_then(Value::as_str).and_then(kind_index), cell.get("color").and_then(Value::as_u64))
            else {
                continue;
            };
            squares[square(x + 1, y + 1)] = kind | (color.min(1) as u8) << 4;
        }
    }
    let mut hands = [[0u8; 7]; 2];
    if let Some(hs) = data.get("hands").and_then(Value::as_array) {
        for (color, h) in hs.iter().take(2).enumerate() {
            let Some(obj) = h.as_object() else { continue };
            for (k, n) in obj {
                let (Some(kind), Some(n)) = (kind_index(k), n.as_u64()) else { continue };
                if let Some(i) = notation::hand_index(kind) {
                    hands[color][i] = n.min(18) as u8;
                }
            }
        }
    }
    board_from_parts(squares, hands, side)
}

fn time_ms(v: Option<&Value>) -> Option<u64> {
    let now = v?.get("now")?;
    let g = |k: &str| now.get(k).and_then(Value::as_u64).unwrap_or(0);
    g("h").checked_mul(3600)?.checked_add(g("m").checked_mul(60)?)?.checked_add(g("s"))?.checked_mul(1000)
}

fn comments(v: &Value) -> Option<String> {
    let list = v.get("comments")?.as_array()?;
    let lines: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Apply one MoveFormat element as a child of `parent`; returns the new node.
fn apply(rec: &mut GameRecord, parent: usize, mf: &Value) -> Result<Option<usize>> {
    let node = if let Some(sp) = mf.get("special").and_then(Value::as_str) {
        let special = match sp {
            "+ILLEGAL_ACTION" | "-ILLEGAL_ACTION" => {
                let offender = u8::from(sp.starts_with('-'));
                if offender == rec.nodes[parent].board.side { Special::IllegalLoss } else { Special::IllegalWin }
            }
            _ => SPECIALS.iter().find(|(c, _)| *c == sp).map(|(_, s)| *s).unwrap_or(Special::Interrupt),
        };
        rec.add_special(parent, special)
    } else if let Some(m) = mf.get("move").filter(|m| !m.is_null()) {
        let to = m.get("to").and_then(sq_of).ok_or_else(|| bad("move.to"))?;
        let piece = m.get("piece").and_then(Value::as_str).and_then(kind_index).ok_or_else(|| bad("move.piece"))?;
        let from = m.get("from").filter(|f| !f.is_null()).map(|f| sq_of(f).ok_or_else(|| bad("move.from"))).transpose()?;
        let promote = m.get("promote").and_then(Value::as_bool).unwrap_or(false);
        let mv = notation::try_make_move(from, to, promote, if from.is_none() { piece } else { 0 })
            .ok_or_else(|| Error::new(ErrorKind::IllegalMove(m.to_string())))?;
        rec.add_move(parent, mv)?
    } else {
        return Ok(None);
    };
    rec.nodes[node].time_ms = time_ms(mf.get("time"));
    if let Some(c) = comments(mf) {
        rec.nodes[node].comment = c;
    }
    Ok(Some(node))
}

pub fn read_jkf(text: &str, limits: &Limits) -> Result<GameRecord> {
    check_text(text, limits)?;
    let v: Value = serde_json::from_str(text).map_err(|e| bad(format!("JSON: {e}")))?;
    let mut rec = GameRecord::new(initial_board(v.get("initial"))?);
    if let Some(h) = v.get("header").and_then(Value::as_object) {
        for (k, val) in h {
            if let Some(s) = val.as_str() {
                rec.headers.push((k.clone(), s.to_owned()));
            }
        }
    }
    let moves = v.get("moves").and_then(Value::as_array).ok_or_else(|| bad("moves"))?;
    if let Some(c) = moves.first().and_then(comments) {
        rec.nodes[0].comment = c;
    }
    // Work stack of (parent of the first move, move list, first index): no recursion on input.
    let mut work: Vec<(usize, &[Value])> = vec![(0, moves.get(1..).unwrap_or(&[]))];
    while let Some((start_parent, list)) = work.pop() {
        let mut parent = start_parent;
        for mf in list {
            if rec.nodes.len() >= limits.max_nodes {
                return Err(ErrorKind::TooLarge { what: "moves", limit: limits.max_nodes }.into());
            }
            if rec.nodes[parent].special.is_some() {
                break;
            }
            // Forks are alternatives to this element: same parent.
            if let Some(forks) = mf.get("forks").and_then(Value::as_array) {
                for f in forks.iter().rev() {
                    if let Some(fl) = f.as_array() {
                        work.push((parent, fl.as_slice()));
                    }
                }
            }
            match apply(&mut rec, parent, mf)? {
                Some(n) => parent = n,
                None => continue,
            }
        }
    }
    // Forks pushed after the main line was built are children added later, so the main line stays first.
    crate::eval_comment::extract_all(&mut rec);
    Ok(rec)
}

/// Comment lines of a node: its own comment plus engine readings in `style` (as in KIF, minus the `*`).
fn comment_lines(rec: &GameRecord, node: usize, style: Option<EvalStyle>) -> Vec<String> {
    let mut lines: Vec<String> = rec.nodes[node].comment.lines().map(str::to_owned).collect();
    if let Some(style) = style {
        lines.extend(crate::eval_comment::kif_lines(rec, node, style));
    }
    lines
}

fn move_json(rec: &GameRecord, node: usize, cum: &[u64], style: Option<EvalStyle>) -> Value {
    let n = &rec.nodes[node];
    let mut obj = Map::new();
    let comments = comment_lines(rec, node, style);
    if !comments.is_empty() {
        obj.insert("comments".into(), json!(comments));
    }
    if let Some(ms) = n.time_ms {
        let s = ms / 1000;
        let t = cum[node] / 1000;
        obj.insert(
            "time".into(),
            json!({"now": {"m": s / 60, "s": s % 60}, "total": {"h": t / 3600, "m": t / 60 % 60, "s": t % 60}}),
        );
    }
    let Some(parent) = n.parent else { return Value::Object(obj) };
    if let Some(sp) = n.special {
        let code = match sp {
            Special::IllegalWin => if rec.nodes[parent].board.side == 0 { "-ILLEGAL_ACTION" } else { "+ILLEGAL_ACTION" },
            other => SPECIALS.iter().find(|(_, s)| *s == other).map(|(c, _)| *c).unwrap_or("CHUDAN"),
        };
        obj.insert("special".into(), json!(code));
        return Value::Object(obj);
    }
    let Some(mv) = n.mv else { return Value::Object(obj) };
    let board = &rec.nodes[parent].board;
    let f = notation::facts(board, mv);
    let mut m = Map::new();
    m.insert("color".into(), json!(f.side));
    if let Some(from) = f.from {
        m.insert("from".into(), json!({"x": file_of(from), "y": rank_of(from)}));
    }
    m.insert("to".into(), json!({"x": file_of(f.to), "y": rank_of(f.to)}));
    m.insert("piece".into(), json!(KINDS[f.kind as usize]));
    if rec.prev_to(parent) == Some(f.to) {
        m.insert("same".into(), json!(true));
    }
    if f.can_promote {
        m.insert("promote".into(), json!(f.promote));
    }
    if f.captured != 0 {
        m.insert("capture".into(), json!(KINDS[f.captured as usize]));
    }
    let legal = board.legal_moves();
    let rel: String = notation::disambiguation(board, &f, &legal)
        .chars()
        .filter_map(|c| match c {
            '左' => Some('L'),
            '直' => Some('C'),
            '右' => Some('R'),
            '上' => Some('U'),
            '寄' => Some('M'),
            '引' => Some('D'),
            '打' => Some('H'),
            _ => None,
        })
        .collect();
    if !rel.is_empty() {
        m.insert("relative".into(), json!(rel));
    }
    obj.insert("move".into(), Value::Object(m));
    Value::Object(obj)
}

/// Depth of nested variations, computed without recursion.
fn fork_depth(rec: &GameRecord) -> usize {
    let mut depth = vec![0usize; rec.nodes.len()];
    let mut max = 0;
    for i in 1..rec.nodes.len() {
        let Some(p) = rec.nodes[i].parent else { continue };
        let is_alt = rec.nodes[p].children.first() != Some(&i);
        depth[i] = depth[p] + usize::from(is_alt);
        max = max.max(depth[i]);
    }
    max
}

pub fn write_jkf(rec: &GameRecord, eval_style: Option<EvalStyle>) -> Result<String> {
    if fork_depth(rec) > MAX_FORK_DEPTH {
        return Err(ErrorKind::TooLarge { what: "variation depth", limit: MAX_FORK_DEPTH }.into());
    }
    let mut header = Map::new();
    for (k, v) in &rec.headers {
        if k != "手合割" {
            header.insert(k.clone(), json!(v));
        }
    }
    let initial = match handicap_name(rec.initial()) {
        Some("平手") if rec.initial().side == 0 => Value::Null,
        Some(name) if rec.initial().side == 1 => {
            json!({"preset": PRESETS.iter().find(|(_, n)| *n == name).map(|(p, _)| *p).unwrap_or("OTHER")})
        }
        _ => {
            let b = rec.initial();
            let board: Vec<Vec<Value>> = (1..=9)
                .map(|x| {
                    (1..=9)
                        .map(|y| {
                            let p = b.squares[square(x, y)];
                            if p == 0 { json!({}) } else { json!({"color": p >> 4, "kind": KINDS[(p & 15) as usize]}) }
                        })
                        .collect()
                })
                .collect();
            let hands: Vec<Value> = (0..2)
                .map(|c| {
                    let mut o = Map::new();
                    for (i, &k) in notation::HAND_KINDS.iter().enumerate() {
                        o.insert(KINDS[k as usize].into(), json!(b.hands[c][i]));
                    }
                    Value::Object(o)
                })
                .collect();
            json!({"preset": "OTHER", "data": {"color": b.side, "board": board, "hands": hands}})
        }
    };
    // Lines are built in post-order (iteratively): a node's line is its element followed by
    // its first child's line, whose first element carries the other children as forks.
    let cum = crate::kif::cumulative_times(rec);
    let mut line_of: Vec<Option<Vec<Value>>> = vec![None; rec.nodes.len()];
    let mut stack = vec![(0usize, false)];
    while let Some((n, done)) = stack.pop() {
        if !done {
            stack.push((n, true));
            stack.extend(rec.nodes[n].children.iter().map(|&c| (c, false)));
            continue;
        }
        let children = &rec.nodes[n].children;
        let mut tail = Vec::new();
        if let Some(&first) = children.first() {
            tail = line_of[first].take().unwrap_or_default();
            let forks: Vec<Value> = children[1..].iter().filter_map(|&a| line_of[a].take()).map(Value::Array).collect();
            if !forks.is_empty()
                && let Some(Value::Object(o)) = tail.first_mut() {
                    o.insert("forks".into(), Value::Array(forks));
                }
        }
        let mut line = Vec::with_capacity(tail.len() + 1);
        if n == 0 {
            let mut o = Map::new();
            let comments = comment_lines(rec, 0, eval_style);
            if !comments.is_empty() {
                o.insert("comments".into(), json!(comments));
            }
            line.push(Value::Object(o));
        } else {
            line.push(move_json(rec, n, &cum, eval_style));
        }
        line.extend(tail);
        line_of[n] = Some(line);
    }
    let moves = line_of[0].take().unwrap_or_default();
    let doc = json!({"header": header, "initial": initial, "moves": moves});
    serde_json::to_string_pretty(&doc).map_err(|e| bad(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_forks_and_specials() {
        let kif = "手合割：平手\n先手：A\n後手：B\n手数----指手---------消費時間--\n*root\n\
   1 ７六歩(77)   ( 0:03/00:00:03)\n   2 ３四歩(33)\n   3 ２二角成(88)+\n*成った\n   4 同　銀(31)\n   5 投了\n\n変化：3手\n   3 ６六歩(67)\n   4 ８四歩(83)\n";
        let rec = crate::kif::read_kif(kif, &Limits::default()).unwrap();
        let text = write_jkf(&rec, None).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v["initial"].is_null());
        assert_eq!(v["moves"][1]["move"]["to"], json!({"x": 7, "y": 6}));
        assert_eq!(v["moves"][3]["move"]["promote"], json!(true));
        assert_eq!(v["moves"][3]["move"]["capture"], json!("KA"));
        assert_eq!(v["moves"][4]["move"]["same"], json!(true));
        assert_eq!(v["moves"][5]["special"], json!("TORYO"));
        assert_eq!(v["moves"][3]["forks"][0][0]["move"]["piece"], json!("FU"));
        let again = read_jkf(&text, &Limits::default()).unwrap();
        assert_eq!(again.nodes.len(), rec.nodes.len());
        let labels = |r: &GameRecord| r.nodes.iter().map(|n| n.label.clone()).collect::<Vec<_>>();
        let mut a = labels(&again);
        let mut b = labels(&rec);
        a.sort();
        b.sort();
        assert_eq!(a, b);
        assert_eq!(again.nodes[again.mainline()[3]].comment, "成った");
        assert_eq!(write_jkf(&again, None).unwrap(), text);
    }

    #[test]
    fn custom_position_and_handicap() {
        let rec = GameRecord::from_sfen("4k4/9/9/9/9/9/9/9/4K4 w 2Pr 1").unwrap();
        let text = write_jkf(&rec, None).unwrap();
        let again = read_jkf(&text, &Limits::default()).unwrap();
        assert_eq!(again.initial().key(), rec.initial().key());
        let h = r#"{"header":{},"initial":{"preset":"KA"},"moves":[{},{"move":{"color":1,"from":{"x":3,"y":3},"to":{"x":3,"y":4},"piece":"FU"}}]}"#;
        let rec = read_jkf(h, &Limits::default()).unwrap();
        assert!(rec.is_handicap());
        assert_eq!(rec.nodes[1].label, "△３四歩");
    }

    #[test]
    fn hostile_json() {
        let l = Limits::default();
        assert!(read_jkf("{", &l).is_err());
        assert!(read_jkf(r#"{"header":{},"moves":[{},{"move":{"color":0,"to":{"x":10,"y":1},"piece":"FU"}}]}"#, &l).is_err());
        assert!(read_jkf(r#"{"header":{},"moves":[{},{"move":{"color":0,"to":{"x":5,"y":5},"piece":"OU"}}]}"#, &l).is_err());
        // Deep nesting is rejected by serde_json's recursion limit, not by a stack overflow.
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(read_jkf(&deep, &l).is_err());
    }

    #[test]
    fn engine_readings_survive_jkf() {
        use crate::record::{EngineEval, Eval};
        let mut rec = crate::format::read_usi("position startpos moves 7g7f 3c3d 2g2f", &Limits::default()).unwrap();
        let reading = |cp: i32, pv: &[&str]| EngineEval {
            eval: Some(Eval { cp, mate: None }),
            depth: Some(12),
            pv: pv.iter().map(|s| s.to_string()).collect(),
            engine: Some("E".into()),
            ..Default::default()
        };
        rec.nodes[1].analysis.black = Some(reading(40, &["7g7f", "3c3d"]));
        rec.nodes[2].analysis.white = Some(reading(-35, &["3c3d", "2g2f"]));
        rec.nodes[3].analysis.analysis = Some(reading(60, &["8c8d"]));
        for style in [EvalStyle::ShogiGui, EvalStyle::ShogiHome, EvalStyle::Floodgate] {
            let text = write_jkf(&rec, Some(style)).unwrap();
            let back = read_jkf(&text, &Limits::default()).unwrap();
            let cp = |n: usize, f: fn(&crate::record::NodeAnalysis) -> &Option<EngineEval>| f(&back.nodes[n].analysis).as_ref().and_then(|e| e.eval).map(|e| e.cp);
            assert_eq!(cp(1, |a| &a.black), Some(40), "{style:?}: {text}");
            assert_eq!(cp(2, |a| &a.white), Some(-35), "{style:?}");
            assert!(back.nodes[1].comment.is_empty(), "{style:?}: readings are not left in the comment");
        }
        // Without a style, nothing is added.
        assert!(!write_jkf(&rec, None).unwrap().contains("comments"));
    }
}
