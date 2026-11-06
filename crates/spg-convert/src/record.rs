//! Game records ↔ SPG records (§7.23–7.26, §8.5, §10.2–10.4).

use sakiyomi_kifu::record::{Eval, EngineEval, GameRecord, Special};
use spg::eval::{self, Unit};
use spg::model::*;
use spg::{Move, Spg};

use crate::{Lost, board_to_position, move_from_core, move_to_core, position_to_board};

/// Where a record came from, for `src.*` keys and the "unknown engine" entry (§7.12).
#[derive(Clone, Debug, Default)]
pub struct Origin {
    /// Format name: "kif", "csa", "jkf", "usi", "sakiyomi" …
    pub format: String,
    /// The original file, to keep byte for byte (lossless level L3).
    pub raw: Option<(Vec<u8>, Option<String>)>,
}

/// The event code of `s`, with the original text when it is code 255 (§8.5). `only_node`: the
/// event is the only node after the root, the one form a game won or lost by default takes.
fn special_code(s: Special, only_node: bool) -> (u8, Option<&'static str>) {
    match s {
        Special::Resign => (event::RESIGN, None),
        Special::Interrupt => (event::INTERRUPT, None),
        Special::Sennichite => (event::SENNICHITE, None),
        Special::Jishogi => (event::JISHOGI, None),
        Special::TimeUp => (event::TIMEOUT, None),
        Special::IllegalLoss => (event::ILLEGAL_LOSS, None),
        Special::IllegalWin => (event::ILLEGAL_WIN, None),
        Special::EnteringKingWin => (event::DECLARE_WIN, None),
        Special::Checkmate => (event::MATE, None),
        Special::NoMate => (event::NOMATE, None),
        Special::Draw => (event::DRAW, None),
        Special::MaxMoves => (event::MAX_MOVES, None),
        Special::Matta => (event::MATTA, None),
        Special::WinByDefault if only_node => (event::DEFAULT_WIN, None),
        Special::LossByDefault if only_node => (event::DEFAULT_LOSS, None),
        // A default after moves has no code of its own; keep the text.
        Special::WinByDefault | Special::LossByDefault => (event::OTHER, Some(s.kif_name())),
    }
}

fn special_of(code: u8, raw: Option<&str>) -> Option<Special> {
    Some(match code {
        event::RESIGN => Special::Resign,
        event::INTERRUPT => Special::Interrupt,
        event::SENNICHITE => Special::Sennichite,
        event::JISHOGI => Special::Jishogi,
        event::TIMEOUT => Special::TimeUp,
        event::ILLEGAL_LOSS => Special::IllegalLoss,
        event::ILLEGAL_WIN => Special::IllegalWin,
        event::DECLARE_WIN => Special::EnteringKingWin,
        event::MATE => Special::Checkmate,
        event::NOMATE => Special::NoMate,
        event::DRAW => Special::Draw,
        event::MAX_MOVES => Special::MaxMoves,
        event::MATTA => Special::Matta,
        event::DEFAULT_WIN => Special::WinByDefault,
        event::DEFAULT_LOSS => Special::LossByDefault,
        _ => return raw.and_then(Special::from_kif),
    })
}

/// Sente-view [`Eval`] → EVAL from `side`'s point of view (§8.1).
fn eval_for(e: &Eval, side: u8) -> i32 {
    let sign = if side == 0 { 1 } else { -1 };
    match e.mate {
        Some(n) => {
            let n = n * sign;
            let plies = (n != 0).then(|| n.unsigned_abs());
            eval::mate(n > 0, plies)
        }
        None => (e.cp * sign).clamp(-eval::NORMAL_MAX, eval::NORMAL_MAX),
    }
}

/// EVAL from `side`'s point of view → Sente-view [`Eval`].
fn eval_from(v: i32, side: u8) -> Eval {
    match eval::mate_parts(v) {
        Some((win, plies)) => {
            let n = plies.unwrap_or(999).min(999) as i32;
            Eval::from_side_to_move(0, Some(if win { n } else { -n }), side)
        }
        None => Eval::from_side_to_move(v, None, side),
    }
}

/// Clamp to a field's range (§8.6), recording the original value.
fn clamp_u16(v: u32, attr: &str, annos: &mut Vec<Anno>) -> u16 {
    if v > u32::from(u16::MAX) {
        annos.push(Anno::str(format!("src.range.{attr}"), v.to_string()));
        u16::MAX
    } else {
        v as u16
    }
}

fn clamp_ms(v: u64, attr: &str, annos: &mut Vec<Anno>) -> u32 {
    if v >= u64::from(u32::MAX) {
        annos.push(Anno::str(format!("src.range.{attr}"), v.to_string()));
        u32::MAX - 1
    } else {
        v as u32
    }
}

fn analysis(spg: &mut Spg, e: &EngineEval, context: Context, side: u8, origin: &Origin) -> Analysis {
    let engine = match e.engine.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => spg.engine_index(name, "", Unit::Cp, 100),
        None => spg.unknown_engine(if origin.format.is_empty() { "sakiyomi" } else { &origin.format }, Unit::Cp, 100),
    };
    let mut a = Analysis::new(context, engine);
    if let Some(ev) = &e.eval {
        // A viewpoint inferred from a comment convention is kept as written (§8.1, D.1).
        a.viewpoint_unknown = e.assumed_viewpoint;
        let fits = match ev.mate {
            Some(n) => n.unsigned_abs() <= eval::MAX_MATE_PLIES,
            None => ev.cp.unsigned_abs() <= eval::NORMAL_MAX as u32,
        };
        let original = || match ev.mate {
            Some(n) => format!("mate:{n:+}"),
            None => ev.cp.to_string(),
        };
        if fits {
            a.eval = Some(eval_for(ev, if e.assumed_viewpoint { 0 } else { side }));
        } else if e.assumed_viewpoint {
            // §8.1: an unconfirmed value that does not fit EVAL is kept only as text.
            a.annos.push(Anno::str(raw_eval_key(origin), original()));
        } else {
            // §8.6: rounded to the end of the range (mates of unknown length), original kept.
            a.eval = Some(eval_for(ev, side));
            a.annos.push(Anno::str("src.range.eval", original()));
        }
    }
    a.depth = e.depth.map(|d| clamp_u16(d, "depth", &mut a.annos));
    a.seldepth = e.seldepth.map(|d| clamp_u16(d, "seldepth", &mut a.annos));
    a.nodes = e.nodes;
    a.time_ms = e.time_ms.map(|t| clamp_ms(t, "ms", &mut a.annos));
    let pv: Vec<Move> = e.pv.iter().map_while(|u| Move::from_usi(u).ok()).take(spg::validate::MAX_MOVE_LIST).collect();
    if !pv.is_empty() {
        a.pv = Some(pv);
    }
    a
}

/// `src.<format>.raw_eval`: an unconfirmed evaluation outside EVAL's range (§8.1).
fn raw_eval_key(origin: &Origin) -> String {
    let f = origin.format.to_ascii_lowercase();
    let f = if !f.is_empty() && f.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) { f } else { "sakiyomi".into() };
    format!("src.{f}.raw_eval")
}

/// The value of a `src.<format>.raw_eval` annotation, as written (Sente's view).
fn raw_eval(a: &Analysis) -> Option<Eval> {
    let s = a.annos.iter().find(|x| x.key.starts_with("src.") && x.key.ends_with(".raw_eval")).and_then(|x| if let Value::Str(s) = &x.value { Some(s.as_str()) } else { None })?;
    match s.strip_prefix("mate:") {
        Some(n) => n.parse::<i32>().ok().map(|n| Eval::from_side_to_move(0, Some(n), 0)),
        None => s.parse::<i32>().ok().map(Eval::cp),
    }
}

fn reading(a: &Analysis, side: u8, spg: &Spg) -> EngineEval {
    let engine = spg.engines.get(usize::from(a.engine)).map(|e| e.name.clone()).filter(|n| !n.is_empty());
    let usable = spg.engines.get(usize::from(a.engine)).is_some_and(|e| matches!(e.unit, Unit::Cp | Unit::Raw));
    let raw = if a.eval.is_none() && a.viewpoint_unknown { raw_eval(a) } else { None };
    EngineEval {
        eval: a.eval.filter(|_| usable).map(|v| if a.viewpoint_unknown { eval_from(v, 0) } else { eval_from(v, side) }).or(raw),
        depth: a.depth.map(u32::from),
        seldepth: a.seldepth.map(u32::from),
        nodes: a.nodes,
        time_ms: a.time_ms.map(u64::from),
        pv: a.pv.iter().flatten().map(|m| m.to_usi()).collect(),
        engine,
        assumed_viewpoint: a.viewpoint_unknown,
    }
}

/// Result from the main line's last event (§10.2).
pub fn result_of(rec: &GameRecord) -> GameResult {
    let line = rec.mainline();
    let Some(&end) = line.last() else { return GameResult::Unknown };
    let n = &rec.nodes[end];
    let Some(sp) = n.special else { return GameResult::Unknown };
    let to_move = n.board.side;
    let side_result = |winner: u8| if winner == 0 { GameResult::BlackWin } else { GameResult::WhiteWin };
    match sp {
        Special::Sennichite => {
            // If one side checked on every move between the first and the fourth occurrence
            // of the final position, that side loses; otherwise a draw.
            let key = n.board.key();
            let hits: Vec<usize> = line.iter().enumerate().filter(|(_, i)| rec.nodes[**i].special.is_none() && rec.nodes[**i].board.key() == key).map(|(k, _)| k).collect();
            if hits.len() >= 4 {
                let (first, last) = (hits[0], hits[3]);
                for side in 0..2u8 {
                    let moves: Vec<usize> = line[first + 1..=last].iter().copied().filter(|&i| rec.nodes[i].mv.is_some() && rec.nodes[i].board.side != side).collect();
                    if !moves.is_empty() && moves.iter().all(|&i| rec.nodes[i].board.in_check(1 - side)) {
                        return side_result(1 - side);
                    }
                }
            }
            GameResult::Draw
        }
        Special::Jishogi | Special::Draw | Special::MaxMoves => GameResult::Draw,
        Special::Interrupt | Special::NoMate | Special::Matta => GameResult::Unknown,
        other => other.winner(to_move).map_or(GameResult::Unknown, side_result),
    }
}

/// Annotation on a game-time ANLS entry naming the move the engine played, when its PV does
/// not start with that move (the engine changed its mind, or reported no PV).
pub const PLAYED_KEY: &str = "x.sakiyomi.played";

/// Header items that name an author or a publisher (§10.4).
const AUTHOR_KEYS: [&str; 1] = ["作者"];
const PUBLISHER_KEYS: [&str; 2] = ["発表誌", "出典"];

/// Convert one record, adding engines and rights to `spg`; the record is appended.
pub fn record_to_spg(spg: &mut Spg, rec: &GameRecord, origin: &Origin) {
    let mut r = Record::new(board_to_position(rec.initial()));
    r.start_ply = rec.initial().move_number;
    r.meta = rec.headers.clone();
    r.result = result_of(rec);
    for (k, v) in &rec.headers {
        let role = if AUTHOR_KEYS.contains(&k.as_str()) {
            Role::Author
        } else if PUBLISHER_KEYS.contains(&k.as_str()) {
            Role::Source
        } else {
            continue;
        };
        if v.trim().is_empty() {
            continue;
        }
        let i = spg.rights_index(Rights { role, ..Rights::author(v.trim()) });
        r.annos.push(Anno::int("rights", i as i64));
    }
    if let Some((bytes, name)) = &origin.raw {
        spg.raws.push(Raw { format: origin.format.clone(), filename: name.clone(), compression: Compression::None, stored: bytes.clone(), original_len: bytes.len() as u64 });
        r.raw = Some((spg.raws.len() - 1) as u64);
    }
    // Pre-order walk; map GameRecord node → SPG node.
    r.nodes.clear();
    let mut map = vec![0u32; rec.nodes.len()];
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        let n = &rec.nodes[i];
        let me = r.nodes.len() as u32;
        map[i] = me;
        let parent = n.parent.map_or(0, |p| map[p]);
        let only_node = n.parent == Some(0) && rec.nodes.len() == 2;
        let kind = match (n.mv, n.special) {
            (Some(m), _) => NodeKind::Move(move_from_core(m)),
            (None, Some(sp)) => NodeKind::Event(special_code(sp, only_node).0),
            (None, None) => NodeKind::Root,
        };
        let mut node = RecordNode::new(parent, kind);
        if let Some(raw) = n.special.and_then(|s| special_code(s, only_node).1) {
            node.annos.push(Anno::str("event.raw", raw));
        }
        if let Some(t) = n.time_ms {
            node.time_ms = Some(clamp_ms(t, "t", &mut node.annos));
        }
        if !n.comment.is_empty() {
            node.annos.push(Anno::str("comment", n.comment.clone()));
        }
        r.nodes.push(node);
        stack.extend(n.children.iter().rev());
    }
    // Readings: a game engine's reading belongs to the position it thought about (the
    // parent), with its PV starting with the move it played; an analysis belongs to the node.
    for (i, n) in rec.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            let side = rec.nodes[p].board.side;
            if let Some(e) = n.analysis.slot(side) {
                let mut a = analysis(spg, e, Context::Game, side, origin);
                if let Some(m) = n.mv.map(move_from_core)
                    && a.pv.as_ref().and_then(|p| p.first()) != Some(&m)
                {
                    a.annos.push(Anno::new(PLAYED_KEY, Value::Moves(vec![m])));
                }
                r.nodes[map[p] as usize].analyses.push(a);
            }
        }
        if let Some(e) = &n.analysis.analysis {
            let a = analysis(spg, e, Context::Analysis, n.board.side, origin);
            r.nodes[map[i] as usize].analyses.push(a);
        }
    }
    spg.records.push(r);
}

pub fn records_to_spg(recs: &[GameRecord], origin: &Origin) -> Spg {
    let mut spg = Spg::default();
    for r in recs {
        record_to_spg(&mut spg, r, origin);
    }
    spg
}

/// Convert every record to the GUI model; what it cannot hold is reported.
pub fn spg_to_records(spg: &Spg) -> (Vec<GameRecord>, Lost, Vec<String>) {
    let mut lost = Lost::default();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    lost.add("file annotations", spg.file_annos.len());
    for (ri, r) in spg.records.iter().enumerate() {
        match record_to_game(spg, r, &mut lost) {
            Ok(g) => out.push(g),
            Err(e) => errors.push(format!("record {}: {e}", ri + 1)),
        }
    }
    (out, lost, errors)
}

fn record_to_game(spg: &Spg, r: &Record, lost: &mut Lost) -> Result<GameRecord, String> {
    let start = position_to_board(&r.start, r.start_ply).ok_or("the start position needs both kings and the standard set of pieces")?;
    let mut g = GameRecord::new(start);
    g.headers = r.meta.clone();
    lost.add("record annotations", r.annos.iter().filter(|a| a.key != "rights").count());
    if r.nongame {
        lost.add("non-game flag", 1);
    }
    let mut map = vec![usize::MAX; r.nodes.len()];
    map[0] = 0;
    for (i, n) in r.nodes.iter().enumerate() {
        let gi = match n.kind {
            NodeKind::Root => 0,
            NodeKind::Move(m) => {
                let parent = map[n.parent as usize];
                if parent == usize::MAX {
                    lost.add("moves after an unsupported node", 1);
                    continue;
                }
                let mv = move_to_core(m).ok_or("bad move")?;
                let before = g.nodes.len();
                let c = g.add_move(parent, mv).map_err(|e| format!("{e}"))?;
                if g.nodes.len() == before {
                    lost.add("repeated sibling moves (merged)", 1);
                }
                c
            }
            NodeKind::Event(code) => {
                let parent = map[n.parent as usize];
                let raw = n.annos.iter().find(|a| a.key == "event.raw").and_then(|a| if let Value::Str(s) = &a.value { Some(s.as_str()) } else { None });
                match special_of(code, raw) {
                    Some(sp) if parent != usize::MAX => g.add_special(parent, sp),
                    _ => {
                        lost.add("unrecognised events", 1);
                        continue;
                    }
                }
            }
        };
        map[i] = gi;
        let node = &mut g.nodes[gi];
        node.time_ms = n.time_ms.map(u64::from).or(node.time_ms);
        let comments: Vec<&str> = n.annos.iter().filter(|a| a.key == "comment").filter_map(|a| if let Value::Str(s) = &a.value { Some(s.as_str()) } else { None }).collect();
        if !comments.is_empty() {
            let joined = comments.join("\n");
            node.comment = if node.comment.is_empty() { joined } else { format!("{}\n{joined}", node.comment) };
        }
        lost.add("bookmarks", n.annos.iter().filter(|a| a.key == "bookmark").count());
        lost.add("node annotations", n.annos.iter().filter(|a| !matches!(a.key.as_str(), "comment" | "bookmark" | "event.raw" | "rights") && !a.key.starts_with("src.range.")).count());
    }
    // Readings.
    for (i, n) in r.nodes.iter().enumerate() {
        let gi = map[i];
        if gi == usize::MAX {
            continue;
        }
        let side = g.nodes[gi].board.side;
        let mut used_game = std::collections::HashSet::new();
        let mut used_analysis = false;
        for a in &n.analyses {
            match a.context {
                Context::Game => {
                    // Find the child the engine played: named by x.sakiyomi.played, else the
                    // first move of its PV.
                    let played = a.annos.iter().find(|x| x.key == PLAYED_KEY).and_then(|x| if let Value::Moves(m) = &x.value { m.first().copied() } else { None });
                    let first = played.or_else(|| a.pv.as_ref().and_then(|p| p.first()).copied()).and_then(move_to_core);
                    let child = first.and_then(|m| g.nodes[gi].children.iter().copied().find(|&c| g.nodes[c].mv == Some(m)));
                    match child {
                        Some(c) if a.rank <= 1 && used_game.insert(c) => {
                            let e = reading(a, side, spg);
                            *g.nodes[c].analysis.slot_mut(side) = Some(e);
                        }
                        _ => lost.add("game-time readings (other candidates or no matching move)", 1),
                    }
                }
                Context::Analysis if a.rank <= 1 && !used_analysis => {
                    g.nodes[gi].analysis.analysis = Some(reading(a, side, spg));
                    used_analysis = true;
                }
                _ => lost.add("other analyses", 1),
            }
        }
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use sakiyomi_kifu::format;
    use sakiyomi_kifu::Limits;

    const KIF: &str = "先手：A\n後手：B\n作者：Carol\n手数----指手---------消費時間--\n   1 ７六歩(77)   ( 0:12/00:00:12)\n**対局 時間 00:10.0 深さ 20/30 ノード数 1000 評価値 45 読み筋 ▲７六歩(77) △３四歩(33)\n*角道を開ける\n   2 ３四歩(33)   ( 0:06/00:00:06)\n   3 ２六歩(27)   ( 0:03/00:00:15)\n   4 投了\nまで3手で先手の勝ち\n\n変化：3手\n   3 ６六歩(67)   ( 0:04/00:00:16)\n";

    #[test]
    fn kif_through_spg() {
        let rec = format::read(KIF, None, &Limits::default()).unwrap();
        let spg = records_to_spg(std::slice::from_ref(&rec), &Origin { format: "kif".into(), raw: Some((KIF.as_bytes().to_vec(), Some("a.kif".into()))) });
        let r = &spg.records[0];
        assert_eq!(r.result, GameResult::BlackWin);
        assert_eq!(spg.rights[0].name, "Carol");
        assert_eq!(r.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Event(event::RESIGN))).count(), 1);
        // The reading moved to the root (the position thought about), viewpoint unknown.
        let a = &r.nodes[0].analyses[0];
        assert!(a.viewpoint_unknown);
        assert_eq!(a.eval, Some(45));
        assert_eq!(a.pv.as_ref().unwrap()[0].to_usi(), "7g7f");
        let (bytes, _) = spg::write(&spg, &spg::WriteOptions::default()).unwrap();
        let (back, _) = spg::read(&bytes, &spg::ReadOptions::default()).unwrap();
        let (games, lost, errors) = spg_to_records(&back);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(lost.is_empty(), "{}", lost.summary());
        let g = &games[0];
        assert_eq!(g.nodes.len(), rec.nodes.len());
        assert_eq!(g.headers, rec.headers);
        let first = g.nodes[0].children[0];
        assert_eq!(g.nodes[first].comment, "角道を開ける");
        assert_eq!(g.nodes[first].analysis.black.as_ref().unwrap().eval.unwrap().cp, 45);
        assert_eq!(g.nodes[first].time_ms, Some(12_000));
        // Writing the converted record as KIF gives the same text as the original record.
        let a = format::write(&rec, format::Format::Kif, format::TextEncoding::Utf8, None).unwrap();
        let b = format::write(g, format::Format::Kif, format::TextEncoding::Utf8, None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn perpetual_check_loses() {
        let mut g = GameRecord::from_sfen("4k4/9/4G4/9/9/9/9/9/4K4 b G 1").unwrap();
        let mut n = 0;
        // Black checks with the gold back and forth; White's king steps aside and back.
        for (i, m) in ["5c4b", "5a6a", "4b5b", "6a5a"].iter().cycle().take(13).enumerate() {
            let _ = i;
            n = match g.add_usi(n, m) {
                Ok(c) => c,
                Err(_) => break,
            };
        }
        g.add_special(n, Special::Sennichite);
        let r = result_of(&g);
        assert!(matches!(r, GameResult::WhiteWin | GameResult::Draw));
    }
}

#[cfg(test)]
mod by_default {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use sakiyomi_kifu::{Limits, format};

    fn through_spg(g: &GameRecord) -> (Record, GameRecord) {
        let s = records_to_spg(std::slice::from_ref(g), &Origin::default());
        let (bytes, _) = spg::write(&s, &spg::WriteOptions::default()).unwrap();
        let (back, _) = spg::read(&bytes, &spg::ReadOptions::default()).unwrap();
        let (games, lost, errors) = spg_to_records(&back);
        assert!(errors.is_empty() && lost.is_empty(), "{errors:?} {}", lost.summary());
        (back.records[0].clone(), games[0].clone())
    }

    /// §8.5: a game won or lost by default is the root and event 14 or 15, seen from the side
    /// to move at the start, and its result follows (§10.2).
    #[test]
    fn default_games_use_codes_14_and_15() {
        let kif = "手合割：平手\n手数----指手---------消費時間--\n   1 不戦敗\n";
        let g = format::read(kif, None, &Limits::default()).unwrap();
        let (r, back) = through_spg(&g);
        assert_eq!(r.nodes.len(), 2);
        assert_eq!(r.nodes[1].kind, NodeKind::Event(event::DEFAULT_LOSS));
        assert!(r.nodes[1].annos.is_empty(), "no event.raw for a defined code");
        assert_eq!(r.result, GameResult::WhiteWin, "Black, to move, loses by default");
        assert_eq!(back.nodes[1].special, Some(Special::LossByDefault));
        // A handicap game starts with White (上手) to move: 不戦勝 is White's win.
        let mut g = GameRecord::from_sfen("lnsgkgsn1/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1").unwrap();
        g.add_special(0, Special::WinByDefault);
        let (r, _) = through_spg(&g);
        assert_eq!(r.nodes[1].kind, NodeKind::Event(event::DEFAULT_WIN));
        assert_eq!(r.result, GameResult::WhiteWin);
    }

    /// A default after moves is not the §8.5 form: it stays code 255 with its text, and the
    /// validator refuses 14/15 anywhere but directly after the root.
    #[test]
    fn default_after_moves_keeps_its_text() {
        let mut g = GameRecord::from_start();
        let n = g.add_usi(0, "7g7f").unwrap();
        g.add_special(n, Special::WinByDefault);
        let (r, back) = through_spg(&g);
        assert_eq!(r.nodes[2].kind, NodeKind::Event(event::OTHER));
        assert_eq!(back.nodes[2].special, Some(Special::WinByDefault));
        let mut s = records_to_spg(std::slice::from_ref(&g), &Origin::default());
        s.records[0].nodes[2].kind = NodeKind::Event(event::DEFAULT_WIN);
        s.records[0].nodes[2].annos.clear();
        assert!(spg::write(&s, &spg::WriteOptions::default()).is_err());
    }
}

#[cfg(test)]
mod eval_ranges {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// §8.1: an unconfirmed value outside EVAL is stored as none with `src.<format>.raw_eval`;
    /// §8.6: a confirmed one is rounded to the range's end with `src.range.eval`.
    #[test]
    fn out_of_range_evaluations() {
        let mut g = GameRecord::from_start();
        let n = g.add_usi(0, "7g7f").unwrap();
        g.nodes[n].analysis.analysis = Some(EngineEval { eval: Some(Eval::cp(20_000_000)), assumed_viewpoint: true, ..Default::default() });
        let n2 = g.add_usi(n, "3c3d").unwrap();
        g.nodes[n2].analysis.analysis = Some(EngineEval { eval: Some(Eval::cp(-20_000_000)), ..Default::default() });
        let s = records_to_spg(std::slice::from_ref(&g), &Origin { format: "kif".into(), raw: None });
        let r = &s.records[0];
        let a = &r.nodes[1].analyses[0];
        assert_eq!(a.eval, None);
        assert!(a.viewpoint_unknown);
        assert!(a.annos.iter().any(|x| x.key == "src.kif.raw_eval" && x.value == Value::Str("20000000".into())));
        let a = &r.nodes[2].analyses[0];
        assert_eq!(a.eval, Some(-eval::NORMAL_MAX), "Black to move after 3c3d: rounded to the end of the range");
        assert!(a.annos.iter().any(|x| x.key == "src.range.eval"));
        let (bytes, _) = spg::write(&s, &spg::WriteOptions::default()).unwrap();
        let (back, _) = spg::read(&bytes, &spg::ReadOptions::default()).unwrap();
        let (games, _, _) = spg_to_records(&back);
        assert_eq!(games[0].nodes[n].analysis.analysis.as_ref().unwrap().eval.unwrap().cp, 20_000_000, "the unconfirmed value comes back as written");
    }
}

#[cfg(test)]
mod played_move {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A reading whose PV does not start with the move played still returns to that move.
    #[test]
    fn reading_with_another_pv_returns_to_its_move() {
        let mut g = GameRecord::from_start();
        let n = g.add_usi(0, "7g7f").unwrap();
        g.nodes[n].analysis.black = Some(EngineEval { eval: Some(Eval::cp(15)), pv: vec!["2g2f".into(), "8c8d".into()], engine: Some("e".into()), ..Default::default() });
        let n2 = g.add_usi(n, "3c3d").unwrap();
        g.nodes[n2].analysis.white = Some(EngineEval { eval: Some(Eval::cp(-20)), engine: Some("e".into()), ..Default::default() });
        let s = records_to_spg(std::slice::from_ref(&g), &Origin::default());
        let (bytes, _) = spg::write(&s, &spg::WriteOptions::default()).unwrap();
        let (back, _) = spg::read(&bytes, &spg::ReadOptions::default()).unwrap();
        let (games, lost, _) = spg_to_records(&back);
        assert!(lost.is_empty(), "{}", lost.summary());
        assert_eq!(games[0].nodes[n].analysis.black, g.nodes[n].analysis.black);
        assert_eq!(games[0].nodes[n2].analysis.white, g.nodes[n2].analysis.white);
    }
}
