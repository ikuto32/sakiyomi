//! Engine evaluations and principal variations stored as record comments, in the
//! conventions other software uses. Reading recognises all of them; writing uses one.
//!
//! | style | KIF comment (after `*`) | score | PV |
//! |---|---|---|---|
//! | ShogiGUI | `**解析 0 ○ 時間 00:01.0 深さ 20/28 ノード数 N 評価値 45 読み筋 ▲２六歩(27) …` (`*対局 …` in games) | Sente | KIF moves with origin |
//! | ShogiHome | `**評価値=45` / `*#評価値=45`, `読み筋=▲７六歩△３四歩`, `深さ=`, `ノード数=`, `エンジン=` | Sente | KI2, no spaces |
//! | Floodgate (CSA) | `'** 45 -3334FU +2726FU #1234` | Sente | CSA moves |
//! | 棋神アナリティクス (read only) | `* Engine NAME 候補1 深さ … 評価値 45 読み筋 …` | Sente | KIF moves |
//!
//! Placement: a game engine's reading is attached to the move it played (its PV starts with
//! that move); an analysis is attached to the position after the move.

use shogi_core::Move16;
use shogi_core::rules::Board;

use crate::csa;
use crate::notation;
use crate::record::{Eval, EngineEval, GameRecord, MATE_CP};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EvalStyle {
    ShogiGui,
    ShogiHome,
    Floodgate,
}

impl EvalStyle {
    pub const ALL: [EvalStyle; 3] = [EvalStyle::ShogiGui, EvalStyle::ShogiHome, EvalStyle::Floodgate];

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "shogigui" => Some(EvalStyle::ShogiGui),
            "shogihome" => Some(EvalStyle::ShogiHome),
            "floodgate" | "csa" => Some(EvalStyle::Floodgate),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EvalStyle::ShogiGui => "shogigui",
            EvalStyle::ShogiHome => "shogihome",
            EvalStyle::Floodgate => "floodgate",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The engine that played the move at this node.
    Mover,
    /// Analysis of the position after this node.
    Analysis,
}

// ---------------------------------------------------------------- reading

/// Parse a score token: `45`, `-120`, `+30`, `45↑`, `-詰3`, `詰`, `+詰`, `30000`.
fn parse_score(tok: &str) -> Option<(Eval, Option<String>)> {
    let bound = if tok.ends_with('↑') {
        Some("lowerbound".to_owned())
    } else if tok.ends_with('↓') {
        Some("upperbound".to_owned())
    } else {
        None
    };
    let t = tok.trim_end_matches(['↑', '↓']);
    if let Some(i) = t.find('詰') {
        let negative = t[..i].contains('-');
        let n: i32 = t[i + '詰'.len_utf8()..].trim().parse().unwrap_or(999);
        let n = n.clamp(0, 999);
        let mate = if negative { -n.max(1) } else { n.max(1) };
        return Some((Eval { cp: if negative { -MATE_CP + n } else { MATE_CP - n }, mate: Some(mate) }, bound));
    }
    let v: i64 = t.trim_start_matches('+').parse().ok()?;
    let v = v.clamp(-(MATE_CP as i64), MATE_CP as i64) as i32;
    let mate = (v.abs() >= MATE_CP - 1000).then(|| if v > 0 { MATE_CP - v } else { -MATE_CP - v });
    Some((Eval { cp: v, mate }, bound))
}

/// KIF-style PV ("▲２六歩(27) △同　歩(23) …") replayed from `board`.
fn parse_kif_pv(board: &Board, prev_to: Option<usize>, text: &str) -> Vec<String> {
    let mut b = *board;
    let mut prev = prev_to;
    let mut out = Vec::new();
    // Re-join "同" with the following token: tokens are separated by ASCII spaces.
    let mut tokens = text.split(' ').filter(|t| !t.is_empty()).peekable();
    while let Some(tok) = tokens.next() {
        let mut t = tok.to_owned();
        if t.trim_start_matches(['▲', '△']) == "同"
            && let Some(next) = tokens.next() {
                t.push_str(next);
            }
        let Ok(mv) = notation::parse_jp_move(&b, &t, prev) else { break };
        out.push(mv.to_usi());
        prev = Some(mv.destination().index());
        b = b.after_generated(mv);
        if out.len() >= 128 {
            break;
        }
    }
    out
}

/// KI2 PV with no separators ("▲７六歩△３四歩▲同　歩").
fn parse_ki2_pv(board: &Board, prev_to: Option<usize>, text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in text.chars() {
        if matches!(c, '▲' | '△' | '☗' | '☖') {
            parts.push(String::new());
        }
        if let Some(p) = parts.last_mut() {
            p.push(c);
        }
    }
    let mut b = *board;
    let mut prev = prev_to;
    let mut out = Vec::new();
    for p in parts.iter().take(128) {
        let Ok(mv) = notation::parse_jp_move(&b, p.trim(), prev) else { break };
        out.push(mv.to_usi());
        prev = Some(mv.destination().index());
        b = b.after_generated(mv);
    }
    out
}

/// CSA PV ("-3334FU +2726FU …") replayed from `board`.
fn parse_csa_pv(board: &Board, tokens: &[&str]) -> Vec<String> {
    let mut b = *board;
    let mut out = Vec::new();
    for t in tokens.iter().take(128) {
        if !t.starts_with(['+', '-']) || t.len() < 7 {
            break;
        }
        let Ok(mv) = csa::parse_csa_move(&b, t) else { break };
        if !b.legal_moves().contains(&mv) {
            break;
        }
        out.push(mv.to_usi());
        b = b.after_generated(mv);
    }
    out
}

/// Context needed to replay PVs for one node.
struct NodeCtx<'a> {
    /// Position before the node's move (the mover's view) and the move itself.
    before: &'a Board,
    played: Option<Move16>,
    before_prev_to: Option<usize>,
    /// Position after the node's move.
    after: &'a Board,
    after_prev_to: Option<usize>,
}

/// ShogiGUI `解析 …` / `対局 …` and 棋神アナリティクス `* Engine …` lines.
fn parse_shogigui(line: &str, ctx: &NodeCtx) -> Option<(Slot, EngineEval)> {
    // In the file these are "**解析 …": the KIF comment marker plus ShogiGUI's own "*".
    let (slot, rest, engine) = if let Some(r) = line.strip_prefix("*解析") {
        (Slot::Analysis, r, None)
    } else if let Some(r) = line.strip_prefix("*対局") {
        (Slot::Mover, r, None)
    } else {
        let r = line.strip_prefix("* Engine ")?;
        let name = r.split(" 候補").next().unwrap_or("").trim().to_owned();
        (Slot::Analysis, r, Some(name))
    };
    if !rest.contains("評価値") && !rest.contains("読み筋") {
        return None;
    }
    let mut e = EngineEval { engine, ..Default::default() };
    let (head, pv_text) = match rest.split_once("読み筋") {
        Some((h, p)) => (h, Some(p.trim())),
        None => (rest, None),
    };
    let toks: Vec<&str> = head.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            "深さ" if i + 1 < toks.len() => {
                let mut it = toks[i + 1].split('/');
                e.depth = it.next().and_then(|d| d.parse().ok());
                e.seldepth = it.next().and_then(|d| d.parse().ok());
                i += 1;
            }
            "ノード数" if i + 1 < toks.len() => {
                e.nodes = toks[i + 1].parse().ok();
                i += 1;
            }
            "時間" if i + 1 < toks.len() => {
                let mut it = toks[i + 1].split(':');
                if let (Some(m), Some(s)) = (it.next().and_then(|m| m.parse::<u64>().ok()), it.next().and_then(|s| s.parse::<f64>().ok()))
                    && s.is_finite() && s >= 0.0 {
                        e.time_ms = m.checked_mul(60_000).map(|ms| ms.saturating_add((s * 1000.0).min(6e7) as u64));
                    }
                i += 1;
            }
            "評価値" if i + 1 < toks.len() => {
                e.eval = parse_score(toks[i + 1]).map(|(v, _)| v);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(pv) = pv_text {
        e.pv = match slot {
            Slot::Analysis => parse_kif_pv(ctx.after, ctx.after_prev_to, pv),
            Slot::Mover => parse_kif_pv(ctx.before, ctx.before_prev_to, pv),
        };
    }
    if slot == Slot::Mover && e.pv.is_empty()
        && let Some(m) = ctx.played {
            e.pv.push(m.to_usi());
        }
    Some((slot, e))
}

/// Floodgate `* 45 -3334FU +2726FU #1234` (KIF `**…` / CSA `'**…`). PV excludes the played move.
fn parse_floodgate(line: &str, ctx: &NodeCtx) -> Option<(Slot, EngineEval)> {
    let rest = line.strip_prefix("* ").or_else(|| line.strip_prefix('*'))?;
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let first = toks.first()?;
    let v: i64 = first.trim_start_matches('+').parse().ok()?;
    let mut e = EngineEval { eval: parse_score(&v.to_string()).map(|(s, _)| s), ..Default::default() };
    let pv_toks: Vec<&str> = toks[1..].iter().copied().take_while(|t| !t.starts_with('#')).collect();
    if let Some(n) = toks.iter().find_map(|t| t.strip_prefix('#')) {
        e.nodes = n.parse().ok();
    }
    e.pv = ctx.played.map(|m| vec![m.to_usi()]).unwrap_or_default();
    e.pv.extend(parse_csa_pv(ctx.after, &pv_toks));
    Some((Slot::Mover, e))
}

/// ShogiHome `*key=value` (game engine) and `#key=value` (analysis) blocks.
fn parse_shogihome(lines: &[&str], ctx: &NodeCtx) -> Vec<(Slot, EngineEval)> {
    let mut out: Vec<(Slot, EngineEval)> = Vec::new();
    let mut current: Option<(Slot, EngineEval)> = None;
    for line in lines {
        let parsed = line
            .strip_prefix('*')
            .map(|r| (Slot::Mover, r))
            .or_else(|| line.strip_prefix('#').map(|r| (Slot::Analysis, r)))
            .and_then(|(slot, r)| r.split_once('=').map(|(k, v)| (slot, k, v)));
        let Some((slot, key, value)) = parsed.filter(|(_, k, _)| !k.is_empty() && !k.contains([' ', '*', '#'])) else {
            if let Some(c) = current.take() {
                out.push(c);
            }
            continue;
        };
        if current.as_ref().is_some_and(|(s, _)| *s != slot) {
            out.extend(current.take());
        }
        let (_, e) = current.get_or_insert_with(|| (slot, EngineEval::default()));
        match key {
            "評価値" => e.eval = parse_score(value.trim()).map(|(v, _)| v),
            "読み筋" => {
                // PV starts with the move after the commented one.
                let pv = parse_ki2_pv(ctx.after, ctx.after_prev_to, value.trim());
                e.pv = match (slot, ctx.played) {
                    (Slot::Mover, Some(m)) => std::iter::once(m.to_usi()).chain(pv).collect(),
                    _ => pv,
                };
            }
            "深さ" => e.depth = value.trim().parse().ok(),
            "ノード数" => e.nodes = value.trim().parse().ok(),
            "エンジン" => e.engine = Some(value.trim().to_owned()),
            "詰み" => {
                let winner_black = value.starts_with("先手");
                let n: i32 = value.split(':').nth(1).map(|s| s.trim_end_matches('手')).and_then(|s| s.parse().ok()).unwrap_or(999);
                let n = n.clamp(1, 999);
                e.eval = Some(Eval { cp: if winner_black { MATE_CP - n } else { -MATE_CP + n }, mate: Some(if winner_black { n } else { -n }) });
            }
            _ => {}
        }
    }
    out.extend(current);
    out
}

fn is_eval_line(line: &str) -> bool {
    line.starts_with("*解析 ")
        || line.starts_with("*対局 ")
        || line.starts_with("*候補手 ")
        || line.starts_with("* Engine ")
        || line.starts_with("*Engines ")
        || (line.starts_with(['*', '#']) && line[1..].split_once('=').is_some_and(|(k, _)| !k.is_empty() && !k.contains(' ')))
        || line.strip_prefix("* ").is_some_and(|r| r.split_whitespace().next().is_some_and(|t| t.trim_start_matches('+').parse::<i64>().is_ok()))
}

/// Move recognised evaluation lines out of every node's comment into its analysis.
/// Unrecognised comment text is kept as is.
pub fn extract_all(rec: &mut GameRecord) {
    for n in 0..rec.nodes.len() {
        if !rec.nodes[n].comment.lines().any(is_eval_line) {
            continue;
        }
        let parent = rec.nodes[n].parent.unwrap_or(n);
        let before = rec.nodes[parent].board;
        let after = rec.nodes[n].board;
        let ctx = NodeCtx {
            before: &before,
            played: rec.nodes[n].mv,
            before_prev_to: rec.prev_to(parent),
            after: &after,
            after_prev_to: rec.prev_to(n),
        };
        let comment = std::mem::take(&mut rec.nodes[n].comment);
        let lines: Vec<&str> = comment.lines().collect();
        let mut kept = Vec::new();
        let mut found: Vec<(Slot, EngineEval)> = Vec::new();
        let mut kv: Vec<&str> = Vec::new();
        for line in &lines {
            let is_kv = line.starts_with(['*', '#']) && line[1..].split_once('=').is_some_and(|(k, _)| !k.is_empty() && !k.contains(' '));
            if is_kv {
                kv.push(line);
                continue;
            }
            if !kv.is_empty() {
                found.extend(parse_shogihome(&kv, &ctx));
                kv.clear();
            }
            if let Some(f) = parse_shogigui(line, &ctx).or_else(|| parse_floodgate(line, &ctx)) {
                found.push(f);
            } else if line.starts_with("*候補手 ") || line.starts_with("*Engines ") {
                // ShogiGUI alternative-move and engine-list lines: informational only.
            } else {
                kept.push(*line);
            }
        }
        if !kv.is_empty() {
            found.extend(parse_shogihome(&kv, &ctx));
        }
        // ShogiHome writes a situation word (互角, 先手有利 …) before its block; drop it with the block.
        rec.nodes[n].comment = kept.join("\n");
        let mover = before.side;
        for (slot, mut e) in found {
            // Every comment convention is read as Sente's view, but none states it.
            e.assumed_viewpoint = true;
            let target = match slot {
                Slot::Analysis => &mut rec.nodes[n].analysis.analysis,
                Slot::Mover if n == 0 => &mut rec.nodes[n].analysis.analysis,
                Slot::Mover => rec.nodes[n].analysis.slot_mut(mover),
            };
            if target.is_none() {
                *target = Some(e);
            }
        }
    }
}

// ---------------------------------------------------------------- writing

fn fmt_score(e: Option<Eval>) -> String {
    match e {
        // Other GUIs read mate scores as ±30000; keep the distance in the cp value.
        Some(e) => e.cp.to_string(),
        None => "0".to_owned(),
    }
}

/// KIF-style PV text from `board` ("▲７六歩(77) △３四歩(33) …").
fn kif_pv(board: &Board, prev_to: Option<usize>, pv: &[String]) -> String {
    let mut b = *board;
    let mut prev = prev_to;
    let mut out = Vec::new();
    for u in pv {
        let Ok(mv) = Move16::from_usi(u) else { break };
        if !b.legal_moves().contains(&mv) {
            break;
        }
        out.push(format!("{}{}", if b.side == 0 { '▲' } else { '△' }, notation::kif_move(&b, mv, prev)));
        prev = Some(mv.destination().index());
        b = b.after_generated(mv);
    }
    out.join(" ")
}

fn csa_pv(board: &Board, pv: &[String]) -> String {
    let mut b = *board;
    let mut out = Vec::new();
    for u in pv {
        let Ok(mv) = Move16::from_usi(u) else { break };
        if !b.legal_moves().contains(&mv) {
            break;
        }
        out.push(csa::csa_move(&b, mv));
        b = b.after_generated(mv);
    }
    out.join(" ")
}

fn shogigui_line(kind: &str, e: &EngineEval, board: &Board, prev_to: Option<usize>) -> String {
    let mut s = format!("*{kind} 0 ");
    if let Some(ms) = e.time_ms {
        s.push_str(&format!(" 時間 {:02}:{:04.1}", ms / 60_000, (ms % 60_000) as f64 / 1000.0));
    }
    if let Some(d) = e.depth {
        s.push_str(&format!(" 深さ {d}/{}", e.seldepth.unwrap_or(d)));
    }
    if let Some(n) = e.nodes {
        s.push_str(&format!(" ノード数 {n}"));
    }
    s.push_str(&format!(" 評価値 {}", fmt_score(e.eval)));
    let pv = kif_pv(board, prev_to, &e.pv);
    if !pv.is_empty() {
        s.push_str(&format!(" 読み筋 {pv}"));
    }
    s
}

fn shogihome_lines(prefix: char, e: &EngineEval, board_after: &Board, prev_to: Option<usize>, pv: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    match e.eval {
        Some(Eval { mate: Some(m), .. }) => {
            v.push(format!("{prefix}詰み={}:{}手", if m > 0 { "先手勝ち" } else { "後手勝ち" }, m.abs()))
        }
        Some(ev) => v.push(format!("{prefix}評価値={}", ev.cp)),
        None => {}
    }
    let text = notation::pv_to_jp(board_after, pv, prev_to);
    if !text.is_empty() {
        v.push(format!("{prefix}読み筋={text}"));
    }
    if let Some(d) = e.depth {
        v.push(format!("{prefix}深さ={d}"));
    }
    if let Some(n) = e.nodes {
        v.push(format!("{prefix}ノード数={n}"));
    }
    if let Some(name) = &e.engine {
        v.push(format!("{prefix}エンジン={name}"));
    }
    v
}

/// Comment lines (without the leading KIF `*`) for the readings of `node`.
pub fn kif_lines(rec: &GameRecord, node: usize, style: EvalStyle) -> Vec<String> {
    let n = &rec.nodes[node];
    let mut out = Vec::new();
    let parent = n.parent.unwrap_or(node);
    let before = &rec.nodes[parent].board;
    let mover = before.side;
    let mover_eval = if node == 0 { None } else if mover == 0 { n.analysis.black.as_ref() } else { n.analysis.white.as_ref() };
    if let Some(e) = mover_eval {
        match style {
            EvalStyle::ShogiGui => out.push(shogigui_line("対局", e, before, rec.prev_to(parent))),
            EvalStyle::ShogiHome => {
                let rest = e.pv.get(1..).unwrap_or(&[]);
                out.extend(shogihome_lines('*', e, &n.board, rec.prev_to(node), rest));
            }
            EvalStyle::Floodgate => {
                let rest = e.pv.get(1..).unwrap_or(&[]);
                let mut s = format!("* {}", fmt_score(e.eval));
                let pv = csa_pv(&n.board, rest);
                if !pv.is_empty() {
                    s.push(' ');
                    s.push_str(&pv);
                }
                if let Some(nodes) = e.nodes {
                    s.push_str(&format!(" #{nodes}"));
                }
                out.push(s);
            }
        }
    }
    if let Some(e) = &n.analysis.analysis {
        match style {
            EvalStyle::ShogiHome => out.extend(shogihome_lines('#', e, &n.board, rec.prev_to(node), &e.pv)),
            // Floodgate has no analysis form; ShogiGUI's is the most widely read.
            EvalStyle::ShogiGui | EvalStyle::Floodgate => out.push(shogigui_line("解析", e, &n.board, rec.prev_to(node))),
        }
    }
    out
}

/// CSA comment lines (with `'`) for `node`.
pub fn csa_lines(rec: &GameRecord, node: usize, style: EvalStyle) -> Vec<String> {
    match style {
        EvalStyle::ShogiHome => kif_lines(rec, node, style).into_iter().map(|l| format!("'*{l}")).collect(),
        // CSA files always use the Floodgate form for the game engines' readings.
        _ => kif_lines(rec, node, EvalStyle::Floodgate)
            .into_iter()
            .filter(|l| l.starts_with("* "))
            .map(|l| format!("'*{l}"))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Limits;
    use crate::{format, kif};

    fn sample() -> GameRecord {
        let mut rec = format::read_usi("position startpos moves 7g7f 3c3d 2g2f", &Limits::default()).unwrap();
        rec.nodes[1].analysis.black = Some(EngineEval {
            eval: Some(Eval::cp(45)),
            depth: Some(20),
            seldepth: Some(28),
            nodes: Some(1234),
            pv: vec!["7g7f".into(), "3c3d".into(), "2g2f".into()],
            engine: Some("A".into()),
            ..Default::default()
        });
        rec.nodes[2].analysis.white = Some(EngineEval { eval: Some(Eval::cp(-30)), pv: vec!["3c3d".into(), "2g2f".into()], ..Default::default() });
        rec.nodes[2].analysis.analysis = Some(EngineEval {
            eval: Some(Eval::from_side_to_move(0, Some(5), 0)),
            depth: Some(30),
            pv: vec!["2g2f".into(), "8c8d".into()],
            ..Default::default()
        });
        rec
    }

    #[test]
    fn every_style_roundtrips_through_kif() {
        for style in EvalStyle::ALL {
            let rec = sample();
            let text = kif::write_kif(&rec, &kif::WriteOptions { encoding_label: "UTF-8", eval_style: Some(style) });
            let back = kif::read_kif(&text, &Limits::default()).unwrap();
            let b1 = back.nodes[1].analysis.black.clone().unwrap_or_else(|| panic!("{style:?}\n{text}"));
            assert_eq!(b1.eval, Some(Eval::cp(45)), "{style:?}");
            assert_eq!(b1.pv[..2], ["7g7f", "3c3d"], "{style:?}\n{text}");
            let w2 = back.nodes[2].analysis.white.clone().unwrap();
            assert_eq!(w2.eval, Some(Eval::cp(-30)));
            let a2 = back.nodes[2].analysis.analysis.clone().unwrap();
            assert!(a2.eval.unwrap().cp > 29000, "{style:?}");
            assert_eq!(a2.pv[0], "2g2f", "{style:?}");
            assert!(back.nodes.iter().all(|n| n.comment.is_empty()), "{style:?}: leftover comment\n{text}");
        }
    }

    #[test]
    fn reads_real_world_lines() {
        let kif = "\
手数----指手---------消費時間--
**Engines 0 白ビール改
   1 ７六歩(77)        ( 0:02/00:00:02)
**解析 0  時間 00:03.8 深さ 22/24 ノード数 2153099 評価値 90↓ 読み筋 △８四歩(83) ▲２六歩(27) △８五歩(84)
**候補手 0  時間 00:03.8 深さ 19/25 ノード数 2154676 評価値 113 読み筋 ▲２六歩(27)
*普通の手
   2 ８四歩(83)        ( 0:02/00:00:02)
** Engine suisho Version Suisho5 候補1 深さ 10/17 ノード数 107646 評価値 -170 読み筋 ▲２六歩(27) △８五歩(84)
   3 ２六歩(27)
*互角
*#評価値=-102
*#読み筋=△８五歩▲２五歩
*#深さ=21
*#エンジン=MyEngine
";
        let rec = kif::read_kif(kif, &Limits::default()).unwrap();
        let a1 = rec.nodes[1].analysis.analysis.clone().unwrap();
        assert_eq!((a1.eval, a1.depth, a1.seldepth, a1.nodes), (Some(Eval::cp(90)), Some(22), Some(24), Some(2153099)));
        assert_eq!(a1.pv, ["8c8d", "2g2f", "8d8e"]);
        assert_eq!(a1.time_ms, Some(3800));
        assert_eq!(rec.nodes[1].comment, "普通の手");
        let a2 = rec.nodes[2].analysis.analysis.clone().unwrap();
        assert_eq!(a2.engine.as_deref(), Some("suisho Version Suisho5"));
        assert_eq!(a2.pv, ["2g2f", "8d8e"]);
        let a3 = rec.nodes[3].analysis.analysis.clone().unwrap();
        assert_eq!(a3.eval, Some(Eval::cp(-102)));
        assert_eq!(a3.pv, ["8d8e", "2f2e"]);
        assert_eq!(a3.engine.as_deref(), Some("MyEngine"));
        assert_eq!(rec.nodes[3].comment, "互角");

        let csa = "PI\n+\n+2726FU,T15\n'** 99 -8384FU +7776FU #555\n-8384FU\n";
        let rec = crate::csa::read_csa(csa, &Limits::default()).unwrap();
        let b = rec.nodes[1].analysis.black.clone().unwrap();
        assert_eq!((b.eval, b.nodes), (Some(Eval::cp(99)), Some(555)));
        assert_eq!(b.pv, ["2g2f", "8c8d", "7g7f"]);
        let out = crate::csa::write_csa(&rec, Some(EvalStyle::Floodgate));
        assert!(out.contains("'** 99 -8384FU +7776FU #555"), "{out}");
    }

    #[test]
    fn garbage_comments_are_kept() {
        let kif = "手数----指手--\n   1 ７六歩(77)\n*解析 は難しい\n**= weird\n* abc\n";
        let rec = kif::read_kif(kif, &Limits::default()).unwrap();
        assert!(rec.nodes[1].comment.contains("解析 は難しい"));
        assert!(rec.nodes[1].analysis.is_empty());
    }
}
