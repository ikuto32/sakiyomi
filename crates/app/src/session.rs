//! Game sessions. Each session owns its record, clocks and engine processes, so any
//! number of games / analyses can run side by side (multi-view). Engines are bound to
//! a session explicitly; nothing is routed by UI focus.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sakiyomi_kifu::eval_comment::EvalStyle;
use sakiyomi_kifu::format::{self, Format, TextEncoding};
use sakiyomi_kifu::notation;
use sakiyomi_kifu::record::{EngineEval, Eval, GameRecord, NodeAnalysis, Special};
use sakiyomi_kifu::shogi_core::rules::Board;
use sakiyomi_kifu::shogi_core::Move16;
use sakiyomi_kifu::Limits;
use sakiyomi_usi::{GoParams, Info, Registry, Score, UsiProcess, parse_info, start_engine};

use crate::i18n::{kifu_error, tr, trf};

/// Engine score → evaluation from Sente's view.
fn eval_of(score: &Score, side_to_move: u8) -> Eval {
    match *score {
        Score::Cp(cp) => Eval::from_side_to_move(cp, None, side_to_move),
        Score::Mate(n) => Eval::from_side_to_move(0, Some(n), side_to_move),
    }
}

fn reading(info: &Info, side_to_move: u8, engine: &str) -> EngineEval {
    EngineEval {
        eval: info.score.as_ref().map(|s| eval_of(s, side_to_move)),
        depth: info.depth,
        seldepth: info.seldepth,
        nodes: info.nodes,
        time_ms: info.time_ms,
        pv: info.pv.iter().take(64).cloned().collect(),
        engine: Some(engine.to_owned()),
        assumed_viewpoint: false,
    }
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// What opening a file produced besides the loaded record.
#[derive(Default)]
pub struct LoadNotes {
    /// Further records of the file (SPG files may hold several).
    pub more: Vec<GameRecord>,
    /// Data the GUI cannot show.
    pub lost: String,
    pub errors: Vec<String>,
}

/// Extra time an engine may overrun its clock (pipe latency, process scheduling).
const ENGINE_GRACE_MS: u64 = 1000;
const READY_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)] // Events are moved once into the sink; boxing buys nothing.
pub enum Event {
    Session(SessionView),
    Thinking(ThinkingView),
    Closed { id: u32 },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerConfig {
    /// "human" or "engine"
    pub kind: String,
    pub engine_id: Option<String>,
    pub name: Option<String>,
    /// Search budget per move instead of the clock (engines only).
    pub nodes: Option<u64>,
    pub depth: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeControl {
    pub main_ms: u64,
    pub byoyomi_ms: u64,
    pub increment_ms: u64,
}

impl TimeControl {
    fn unlimited(&self) -> bool {
        self.main_ms == 0 && self.byoyomi_ms == 0 && self.increment_ms == 0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameConfig {
    pub players: [PlayerConfig; 2],
    pub times: [TimeControl; 2],
    /// None: continue from the current cursor. Some: start a new record from this SFEN.
    pub start_sfen: Option<String>,
    #[serde(default)]
    pub max_moves: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Black,
    White,
    Analysis,
}

struct Slot {
    key: u64,
    proc: UsiProcess,
    name: String,
    role: Role,
    searching: bool,
    /// Number of `bestmove` replies still owed to searches we abandoned with `stop`.
    stale: u32,
    infos: BTreeMap<u32, Info>,
    search_node: usize,
}

struct Player {
    kind: String,
    name: String,
    key: Option<u64>,
    nodes: Option<u64>,
    depth: Option<u32>,
}

struct Game {
    players: [Player; 2],
    times: [TimeControl; 2],
    remaining: [u64; 2],
    turn_started: Instant,
    tip: usize,
    over: bool,
    result: Option<String>,
    max_moves: u32,
}

struct Analysis {
    key: u64,
    engine: String,
    multipv: u32,
    node: usize,
    batch_ms: Option<u64>,
}

struct Inner {
    id: u32,
    title: String,
    record: GameRecord,
    cursor: usize,
    game: Option<Game>,
    analysis: Option<Analysis>,
    slots: Vec<Slot>,
    graveyard: Vec<UsiProcess>,
    status: String,
    busy: bool,
    dirty_thinking: BTreeSet<u64>,
    next_key: u64,
}

// ------------------------------------------------------------------ views

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub ply: u32,
    pub label: String,
    pub western: String,
    pub special: Option<Special>,
    pub comment: String,
    pub time_ms: Option<u64>,
    /// Readings of Sente's engine, Gote's engine and the analysis engine.
    pub analysis: NodeAnalysis,
    pub usi: Option<String>,
    pub from: Option<usize>,
    pub to: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardView {
    pub squares: Vec<u8>,
    pub hands: [[u8; 7]; 2],
    pub side: u8,
    pub sfen: String,
    pub in_check: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    pub players: [PlayerView; 2],
    pub times: [TimeControl; 2],
    pub remaining_ms: [u64; 2],
    /// Side whose clock is running, if any.
    pub running: Option<u8>,
    /// Milliseconds already spent on the current turn when this view was built.
    pub elapsed_ms: u64,
    pub over: bool,
    pub result: Option<String>,
    pub tip: usize,
    pub human_to_move: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisView {
    pub engine: String,
    pub multipv: u32,
    pub node: usize,
    pub batch: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: u32,
    pub title: String,
    pub nodes: Vec<NodeView>,
    pub cursor: usize,
    pub board: BoardView,
    pub legal: Vec<String>,
    pub game: Option<GameView>,
    pub analysis: Option<AnalysisView>,
    pub status: String,
    pub busy: bool,
    pub black_name: String,
    pub white_name: String,
    pub handicap: bool,
    pub headers: Vec<(String, String)>,
    /// Engine processes owned by this session.
    pub engines: Vec<EngineSlotView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSlotView {
    pub key: u64,
    pub role: Role,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PvLine {
    pub multipv: u32,
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub time_ms: Option<u64>,
    pub hashfull: Option<u32>,
    /// Black's point of view.
    pub eval: Option<Eval>,
    pub bound: Option<String>,
    pub pv: String,
    pub pv_western: String,
    pub pv_usi: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingView {
    pub session: u32,
    pub key: u64,
    pub role: Role,
    pub engine: String,
    pub node: usize,
    pub searching: bool,
    pub lines: Vec<PvLine>,
}

fn now_string() -> String {
    chrono::Local::now().format("%Y/%m/%d %H:%M:%S").to_string()
}

fn result_text(special: Special, side_to_move: u8, handicap: bool) -> String {
    let names = if handicap { [tr("下手"), tr("上手")] } else { [tr("先手"), tr("後手")] };
    let stm = names[side_to_move as usize];
    let other = names[1 - side_to_move as usize];
    let a: [(&str, &dyn std::fmt::Display); 2] = [("stm", &stm), ("other", &other)];
    match special {
        Special::Resign => trf("{other}の勝ち（{stm}の投了）", &a),
        Special::Checkmate => trf("{other}の勝ち（詰み）", &a),
        Special::TimeUp => trf("{other}の勝ち（{stm}の時間切れ）", &a),
        Special::IllegalLoss => trf("{other}の勝ち（{stm}の反則）", &a),
        Special::IllegalWin => trf("{stm}の勝ち（{other}の反則）", &a),
        Special::EnteringKingWin => trf("{stm}の勝ち（入玉宣言）", &a),
        Special::Sennichite => tr("千日手（引き分け）").to_owned(),
        Special::Jishogi => tr("持将棋（引き分け）").to_owned(),
        Special::Draw | Special::MaxMoves => trf("引き分け（{what}）", &[("what", &tr(special.kif_name()))]),
        other => tr(other.kif_name()).to_owned(),
    }
}

impl Inner {
    fn board_at(&self, node: usize) -> &Board {
        &self.record.nodes[node].board
    }

    fn slot_index(&self, key: u64) -> Option<usize> {
        self.slots.iter().position(|s| s.key == key)
    }

    fn send(&mut self, key: u64, line: &str) {
        if let Some(i) = self.slot_index(key) {
            let _ = self.slots[i].proc.send(line);
        }
    }

    /// Remove an engine; it is quit in the background by the caller.
    fn retire(&mut self, key: u64, gameover: Option<&str>) {
        if let Some(i) = self.slot_index(key) {
            let mut slot = self.slots.remove(i);
            if slot.searching {
                let _ = slot.proc.send("stop");
            }
            if let Some(r) = gameover {
                let _ = slot.proc.send(&format!("gameover {r}"));
            }
            self.dirty_thinking.remove(&key);
            self.graveyard.push(slot.proc);
        }
    }

    fn game_active(&self) -> bool {
        self.game.as_ref().is_some_and(|g| !g.over)
    }

    fn new_key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    // ---------------------------------------------------------- game flow

    fn advance(&mut self) {
        let Some(game) = &self.game else { return };
        if game.over {
            return;
        }
        let tip = game.tip;
        let side = self.board_at(tip).side as usize;
        let p = &game.players[side];
        let Some(key) = p.key else { return };
        let go = if let Some(n) = p.nodes {
            GoParams { nodes: Some(n), ..Default::default() }
        } else if let Some(d) = p.depth {
            GoParams { depth: Some(d), ..Default::default() }
        } else if game.times[side].unlimited() {
            GoParams { infinite: true, ..Default::default() }
        } else {
            let inc = game.times.iter().any(|t| t.increment_ms > 0);
            GoParams {
                btime: Some(game.remaining[0]),
                wtime: Some(game.remaining[1]),
                byoyomi: Some(game.times[side].byoyomi_ms),
                binc: inc.then_some(game.times[0].increment_ms),
                winc: inc.then_some(game.times[1].increment_ms),
                ..Default::default()
            }
        };
        let position = format::usi_position(&self.record, tip);
        let Some(i) = self.slot_index(key) else { return };
        let slot = &mut self.slots[i];
        if slot.searching {
            return;
        }
        slot.infos.clear();
        slot.search_node = tip;
        slot.searching = true;
        let _ = slot.proc.send(&position);
        let _ = slot.proc.send(&go.command());
    }

    /// Charge `elapsed` to the side to move; returns false if its flag fell.
    fn charge_clock(&mut self, elapsed: u64, grace: u64) -> bool {
        let tip_side = self.game.as_ref().map(|g| self.board_at(g.tip).side as usize).unwrap_or(0);
        let Some(game) = self.game.as_mut() else { return true };
        let tc = game.times[tip_side];
        if tc.unlimited() {
            return true;
        }
        let rem = &mut game.remaining[tip_side];
        if elapsed <= *rem {
            *rem -= elapsed;
        } else {
            let over = elapsed - *rem;
            *rem = 0;
            if over > tc.byoyomi_ms + grace {
                return false;
            }
        }
        *rem += tc.increment_ms;
        true
    }

    fn play_game_move(&mut self, mv: Move16, reading: Option<EngineEval>, grace: u64) {
        let Some(game) = &self.game else { return };
        let elapsed = game.turn_started.elapsed().as_millis() as u64;
        let tip = game.tip;
        if !self.charge_clock(elapsed, grace) {
            self.finish(Special::TimeUp);
            return;
        }
        let mover = self.board_at(tip).side;
        let node = match self.record.add_move(tip, mv) {
            Ok(n) => n,
            Err(_) => {
                self.finish(Special::IllegalLoss);
                return;
            }
        };
        self.record.nodes[node].time_ms = Some(elapsed);
        if reading.is_some() {
            *self.record.nodes[node].analysis.slot_mut(mover) = reading;
        }
        let Some(game) = self.game.as_mut() else { return };
        game.tip = node;
        game.turn_started = Instant::now();
        let max_moves = game.max_moves;
        if self.cursor == tip {
            self.cursor = node;
        }
        let board = *self.board_at(node);
        if board.legal_moves().is_empty() {
            self.finish(Special::Checkmate);
            return;
        }
        if let Some(sp) = self.repetition(node) {
            self.finish(sp);
            return;
        }
        if max_moves > 0 && self.record.nodes[node].ply >= max_moves {
            self.finish(Special::Jishogi);
            return;
        }
        self.advance();
    }

    /// Fourfold repetition: 千日手, or a perpetual-check loss.
    fn repetition(&self, node: usize) -> Option<Special> {
        let path = self.record.path(node);
        let key = self.board_at(node).key();
        let occ: Vec<usize> = path
            .iter()
            .enumerate()
            .filter(|(_, n)| self.board_at(**n).key() == key)
            .map(|(i, _)| i)
            .collect();
        if occ.len() < 4 {
            return None;
        }
        let cycle = &path[occ[occ.len() - 2] + 1..];
        let mover = 1 - self.board_at(node).side; // side that just moved
        let all_checks = |side: u8| {
            cycle
                .iter()
                .filter(|&&n| self.record.nodes[n].parent.is_some_and(|p| self.board_at(p).side == side))
                .all(|&n| self.board_at(n).in_check(1 - side))
        };
        if all_checks(mover) {
            Some(Special::IllegalWin)
        } else if all_checks(1 - mover) {
            Some(Special::IllegalLoss)
        } else {
            Some(Special::Sennichite)
        }
    }

    fn finish(&mut self, special: Special) {
        let Some(game) = self.game.as_ref() else { return };
        if game.over {
            return;
        }
        let tip = game.tip;
        let elapsed = game.turn_started.elapsed().as_millis() as u64;
        let side = self.board_at(tip).side;
        let handicap = self.record.is_handicap();
        let node = self.record.add_special(tip, special);
        self.record.nodes[node].time_ms = Some(elapsed);
        if self.cursor == tip {
            self.cursor = node;
        }
        let win = special.winner(side);
        let text = result_text(special, side, handicap);
        let keys: Vec<(u64, Role)> = self
            .slots
            .iter()
            .filter(|s| s.role != Role::Analysis)
            .map(|s| (s.key, s.role))
            .collect();
        for (key, role) in keys {
            let me = if role == Role::Black { 0 } else { 1 };
            let r = match win {
                Some(w) if w == me => "win",
                Some(_) => "lose",
                None => "draw",
            };
            self.retire(key, Some(r));
        }
        let Some(game) = self.game.as_mut() else { return };
        game.tip = node;
        game.over = true;
        game.result = Some(text.clone());
        self.status = trf("対局終了: {text}", &[("text", &text)]);
    }

    fn engine_moved(&mut self, key: u64, text: &str) {
        let Some(i) = self.slot_index(key) else { return };
        let role = self.slots[i].role;
        let side = if role == Role::Black { 0 } else { 1 };
        let Some(game) = &self.game else { return };
        if game.over || self.board_at(game.tip).side != side {
            return;
        }
        let tip = game.tip;
        let name = self.slots[i].name.clone();
        let reading = self.slots[i].infos.get(&1).map(|info| reading(info, side, &name));
        match text {
            "resign" => self.finish(Special::Resign),
            "win" => {
                if self.board_at(tip).declaration_win() {
                    self.finish(Special::EnteringKingWin)
                } else {
                    self.finish(Special::IllegalLoss)
                }
            }
            _ => match Move16::from_usi(text) {
                Ok(mv) => self.play_game_move(mv, reading, ENGINE_GRACE_MS),
                Err(_) => self.finish(Special::IllegalLoss),
            },
        }
    }

    fn check_flag(&mut self) {
        let Some(game) = &self.game else { return };
        if game.over {
            return;
        }
        let side = self.board_at(game.tip).side as usize;
        let tc = game.times[side];
        if tc.unlimited() {
            return;
        }
        let grace = if game.players[side].key.is_some() {
            if game.players[side].nodes.is_some() || game.players[side].depth.is_some() {
                return;
            }
            ENGINE_GRACE_MS
        } else {
            0
        };
        let elapsed = game.turn_started.elapsed().as_millis() as u64;
        if elapsed > game.remaining[side] + tc.byoyomi_ms + grace {
            self.finish(Special::TimeUp);
        }
    }

    // ---------------------------------------------------------- analysis

    fn analyze_at(&mut self, node: usize) {
        let Some(a) = &self.analysis else { return };
        let (key, batch) = (a.key, a.batch_ms);
        let position = format::usi_position(&self.record, node);
        let can_move = !self.board_at(node).legal_moves().is_empty();
        let Some(i) = self.slot_index(key) else { return };
        let slot = &mut self.slots[i];
        if slot.searching {
            let _ = slot.proc.send("stop");
            slot.stale += 1;
            slot.searching = false;
        }
        slot.infos.clear();
        slot.search_node = node;
        self.dirty_thinking.insert(key);
        if let Some(a) = self.analysis.as_mut() {
            a.node = node;
        }
        if !can_move {
            if batch.is_some() {
                self.batch_step();
            }
            return;
        }
        let slot = &mut self.slots[i];
        let go = match batch {
            Some(ms) => GoParams { btime: Some(0), wtime: Some(0), byoyomi: Some(ms), ..Default::default() },
            None => GoParams { infinite: true, ..Default::default() },
        };
        slot.searching = true;
        let _ = slot.proc.send(&position);
        let _ = slot.proc.send(&go.command());
    }

    /// Batch analysis: the current node is done, move on along the main line.
    fn batch_step(&mut self) {
        let Some(a) = &self.analysis else { return };
        let next = self.record.nodes[a.node]
            .children
            .first()
            .copied()
            .filter(|&c| self.record.nodes[c].special.is_none());
        match next {
            Some(n) => {
                self.cursor = n;
                self.analyze_at(n);
            }
            None => {
                let key = a.key;
                self.analysis = None;
                self.retire(key, None);
                self.status = tr("棋譜解析が完了しました").to_owned();
            }
        }
    }

    fn cursor_changed(&mut self) {
        if let Some(a) = &self.analysis
            && a.batch_ms.is_none() && a.node != self.cursor {
                self.analyze_at(self.cursor);
            }
    }

    fn stop_analysis(&mut self) {
        if let Some(a) = self.analysis.take() {
            self.retire(a.key, None);
        }
    }

    // ---------------------------------------------------------- engine I/O

    fn on_line(&mut self, key: u64, line: &str) {
        let Some(i) = self.slot_index(key) else { return };
        if line.starts_with("info") {
            if self.slots[i].stale > 0 || !self.slots[i].searching {
                return;
            }
            let Some(info) = parse_info(line) else { return };
            if info.score.is_none() && info.pv.is_empty() {
                // Progress-only line: refresh counters of the first PV.
                if let Some(first) = self.slots[i].infos.get_mut(&1) {
                    first.nodes = info.nodes.or(first.nodes);
                    first.nps = info.nps.or(first.nps);
                    first.time_ms = info.time_ms.or(first.time_ms);
                    first.hashfull = info.hashfull.or(first.hashfull);
                    self.dirty_thinking.insert(key);
                }
                return;
            }
            let mpv = info.multipv.unwrap_or(1);
            if mpv == 1 && self.slots[i].role == Role::Analysis && info.score.is_some() {
                let node = self.slots[i].search_node;
                let side = self.board_at(node).side;
                let name = self.slots[i].name.clone();
                self.record.nodes[node].analysis.analysis = Some(reading(&info, side, &name));
            }
            if mpv == 1 {
                // A new main PV usually means the other lines are being recomputed.
                let max = self.analysis.as_ref().map(|a| a.multipv).unwrap_or(1);
                self.slots[i].infos.retain(|&k, _| k <= max);
            }
            self.slots[i].infos.insert(mpv, info);
            self.dirty_thinking.insert(key);
        } else if let Some(rest) = line.strip_prefix("bestmove") {
            let slot = &mut self.slots[i];
            if slot.stale > 0 {
                slot.stale -= 1;
                return;
            }
            if !slot.searching {
                return;
            }
            slot.searching = false;
            let role = slot.role;
            self.dirty_thinking.insert(key);
            let mv = rest.split_whitespace().next().unwrap_or("resign").to_owned();
            match role {
                Role::Black | Role::White => self.engine_moved(key, &mv),
                Role::Analysis => {
                    if self.analysis.as_ref().is_some_and(|a| a.batch_ms.is_some()) {
                        self.batch_step();
                    }
                }
            }
        }
    }

    fn on_exit(&mut self, key: u64) {
        let Some(i) = self.slot_index(key) else { return };
        let role = self.slots[i].role;
        let name = self.slots[i].name.clone();
        self.slots.remove(i);
        self.dirty_thinking.remove(&key);
        match role {
            Role::Analysis => {
                self.analysis = None;
                self.status = trf("検討エンジン {name} が終了しました", &[("name", &name)]);
            }
            _ => {
                if self.game_active() {
                    self.finish(Special::Interrupt);
                    self.status = trf("エンジン {name} が異常終了したため対局を中断しました", &[("name", &name)]);
                }
            }
        }
    }

    // ---------------------------------------------------------- views

    fn view(&self) -> SessionView {
        let nodes = self
            .record
            .nodes
            .iter()
            .map(|n| NodeView {
                parent: n.parent,
                children: n.children.clone(),
                ply: n.ply,
                label: n.label.clone(),
                western: n.western.clone(),
                special: n.special,
                comment: n.comment.clone(),
                time_ms: n.time_ms,
                analysis: n.analysis.clone(),
                usi: n.mv.map(|m| m.to_usi()),
                from: n.mv.and_then(|m| m.source()).map(|s| s.index()),
                to: n.mv.map(|m| m.destination().index()),
            })
            .collect();
        let node = &self.record.nodes[self.cursor];
        let b = &node.board;
        let game_locks_board = self.game.as_ref().is_some_and(|g| {
            !g.over && (self.cursor != g.tip || g.players[b.side as usize].key.is_some())
        });
        let legal = if node.special.is_some() || game_locks_board || self.busy {
            Vec::new()
        } else {
            b.legal_moves().into_iter().map(|m| m.to_usi()).collect()
        };
        let game = self.game.as_ref().map(|g| {
            let side = self.board_at(g.tip).side;
            GameView {
                players: [0, 1].map(|s| PlayerView { kind: g.players[s].kind.clone(), name: g.players[s].name.clone() }),
                times: g.times,
                remaining_ms: g.remaining,
                running: (!g.over).then_some(side),
                elapsed_ms: if g.over { 0 } else { g.turn_started.elapsed().as_millis() as u64 },
                over: g.over,
                result: g.result.clone(),
                tip: g.tip,
                human_to_move: !g.over && g.players[side as usize].key.is_none(),
            }
        });
        SessionView {
            id: self.id,
            title: self.title.clone(),
            nodes,
            cursor: self.cursor,
            board: BoardView {
                squares: b.squares.to_vec(),
                hands: b.hands,
                side: b.side,
                sfen: b.to_sfen(),
                in_check: b.in_check(b.side),
            },
            legal,
            game,
            analysis: self.analysis.as_ref().map(|a| AnalysisView {
                engine: a.engine.clone(),
                multipv: a.multipv,
                node: a.node,
                batch: a.batch_ms.is_some(),
            }),
            status: self.status.clone(),
            busy: self.busy,
            black_name: self.record.black_name().unwrap_or("").to_owned(),
            white_name: self.record.white_name().unwrap_or("").to_owned(),
            handicap: self.record.is_handicap(),
            headers: self.record.headers.clone(),
            engines: self
                .slots
                .iter()
                .map(|s| EngineSlotView { key: s.key, role: s.role, name: s.name.clone() })
                .collect(),
        }
    }

    fn thinking(&self, key: u64) -> Option<ThinkingView> {
        let slot = self.slots.iter().find(|s| s.key == key)?;
        let node = slot.search_node.min(self.record.nodes.len() - 1);
        let board = self.board_at(node);
        let prev = self.record.prev_to(node);
        let lines = slot
            .infos
            .iter()
            .map(|(&mpv, info)| PvLine {
                multipv: mpv,
                depth: info.depth,
                seldepth: info.seldepth,
                nodes: info.nodes,
                nps: info.nps,
                time_ms: info.time_ms,
                hashfull: info.hashfull,
                eval: info.score.as_ref().map(|s| eval_of(s, board.side)),
                bound: info.bound.clone(),
                pv: notation::pv_to_jp(board, &info.pv, prev),
                pv_western: notation::pv_to_western(board, &info.pv),
                pv_usi: info.pv.clone(),
            })
            .collect();
        Some(ThinkingView {
            session: self.id,
            key,
            role: slot.role,
            engine: slot.name.clone(),
            node,
            searching: slot.searching,
            lines,
        })
    }
}

// ------------------------------------------------------------------ session

pub struct Session {
    pub id: u32,
    inner: Mutex<Inner>,
    sink: Sink,
    closed: AtomicBool,
}

impl Session {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reap retired engines and publish the new state.
    fn publish(&self, g: &mut Inner) {
        let dead = std::mem::take(&mut g.graveyard);
        if !dead.is_empty() {
            thread::spawn(move || {
                for p in dead {
                    p.quit();
                }
            });
        }
        for key in std::mem::take(&mut g.dirty_thinking) {
            if let Some(t) = g.thinking(key) {
                (self.sink)(Event::Thinking(t));
            }
        }
        (self.sink)(Event::Session(g.view()));
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R, String>) -> Result<R, String> {
        let mut g = self.lock();
        let r = f(&mut g);
        self.publish(&mut g);
        r
    }

    pub fn view(&self) -> SessionView {
        self.lock().view()
    }

    /// (engine name, process id) of every engine this session runs.
    pub fn engine_processes(&self) -> Vec<(String, u32)> {
        self.lock().slots.iter().map(|s| (s.name.clone(), s.proc.pid())).collect()
    }

    pub fn thinking_views(&self) -> Vec<ThinkingView> {
        let g = self.lock();
        g.slots.iter().filter_map(|s| g.thinking(s.key)).collect()
    }

    fn tick(&self) {
        let mut g = self.lock();
        let was_over = g.game.as_ref().map(|x| x.over);
        g.check_flag();
        let changed = g.game.as_ref().map(|x| x.over) != was_over;
        if changed {
            self.publish(&mut g);
            return;
        }
        let dirty = std::mem::take(&mut g.dirty_thinking);
        for key in dirty {
            if let Some(t) = g.thinking(key) {
                (self.sink)(Event::Thinking(t));
            }
        }
    }

    fn spawn_pump(self: &Arc<Self>, key: u64, rx: Receiver<String>) {
        let weak = Arc::downgrade(self);
        thread::spawn(move || {
            for line in rx.iter() {
                let Some(s) = weak.upgrade() else { return };
                let mut g = s.lock();
                let before = (g.record.nodes.len(), g.game.as_ref().map(|x| (x.tip, x.over)), g.analysis.as_ref().map(|a| a.node));
                g.on_line(key, &line);
                let after = (g.record.nodes.len(), g.game.as_ref().map(|x| (x.tip, x.over)), g.analysis.as_ref().map(|a| a.node));
                if before != after || !g.graveyard.is_empty() {
                    s.publish(&mut g);
                }
            }
            if let Some(s) = weak.upgrade() {
                let mut g = s.lock();
                if g.slot_index(key).is_some() {
                    g.on_exit(key);
                    s.publish(&mut g);
                }
            }
        });
    }

    fn install_engine(self: &Arc<Self>, g: &mut Inner, mut proc: UsiProcess, name: String, role: Role) -> u64 {
        let key = g.new_key();
        if let Some(rx) = proc.take_receiver() {
            self.spawn_pump(key, rx);
        }
        g.slots.push(Slot { key, proc, name, role, searching: false, stale: 0, infos: BTreeMap::new(), search_node: 0 });
        key
    }

    // ---------------------------------------------------------- commands

    pub fn set_title(&self, title: &str) -> Result<(), String> {
        self.with(|g| {
            g.title = title.to_owned();
            Ok(())
        })
    }

    pub fn set_cursor(&self, node: usize) -> Result<(), String> {
        self.with(|g| {
            if node >= g.record.nodes.len() {
                return Err(tr("存在しない局面です").into());
            }
            if g.analysis.as_ref().is_some_and(|a| a.batch_ms.is_some()) {
                return Err(tr("棋譜解析中は移動できません").into());
            }
            g.cursor = node;
            g.cursor_changed();
            Ok(())
        })
    }

    pub fn make_move(&self, usi: &str) -> Result<(), String> {
        self.with(|g| {
            let mv = Move16::from_usi(usi).map_err(|e| e.to_string())?;
            if g.busy {
                return Err(tr("エンジン起動中です").into());
            }
            if let Some(game) = g.game.as_ref().filter(|x| !x.over) {
                if g.cursor != game.tip {
                    return Err(tr("対局中は最新の局面でのみ指せます").into());
                }
                let side = g.board_at(game.tip).side as usize;
                if game.players[side].key.is_some() {
                    return Err(tr("エンジンの手番です").into());
                }
                if !g.board_at(game.tip).legal_moves().contains(&mv) {
                    return Err(tr("非合法手です").into());
                }
                g.play_game_move(mv, None, 0);
                return Ok(());
            }
            let node = g.record.add_move(g.cursor, mv).map_err(|e| kifu_error(&e))?;
            g.cursor = node;
            g.cursor_changed();
            Ok(())
        })
    }

    pub fn new_game(self: &Arc<Self>, cfg: GameConfig, registry: &Registry) -> Result<(), String> {
        // Phase 1: validate and tear down, under the lock.
        let mut engines = Vec::new();
        for (side, p) in cfg.players.iter().enumerate() {
            if p.kind == "engine" {
                let id = p.engine_id.as_deref().ok_or(tr("エンジンが選択されていません"))?;
                let ec = registry.get(id).ok_or(tr("登録されていないエンジンです"))?;
                engines.push((side, ec));
            }
        }
        if let Some(s) = &cfg.start_sfen {
            Board::from_sfen(s).map_err(|e| trf("開始局面が不正です: {err}", &[("err", &e)]))?;
        }
        self.with(|g| {
            if g.busy {
                return Err(tr("エンジン起動中です").into());
            }
            if g.game_active() {
                return Err(tr("対局中です。先に中断してください").into());
            }
            g.stop_analysis();
            let keys: Vec<u64> = g.slots.iter().map(|s| s.key).collect();
            for k in keys {
                g.retire(k, None);
            }
            g.busy = !engines.is_empty();
            g.status = if engines.is_empty() { String::new() } else { tr("エンジンを起動しています…").into() };
            Ok(())
        })?;
        // Phase 2: start engines without holding the lock (eval files may take a while).
        let mut started = Vec::new();
        for (side, ec) in &engines {
            match start_engine(Path::new(&ec.path), &ec.effective_values(&[]), READY_TIMEOUT) {
                Ok((p, _)) => started.push((*side, ec.name.clone(), p)),
                Err(e) => {
                    let msg = trf("{name} を起動できません: {err}", &[("name", &ec.name), ("err", &e)]);
                    return self.with(|g| {
                        g.busy = false;
                        g.status = msg.clone();
                        Err(msg.clone())
                    });
                }
            }
        }
        // Phase 3: install engines and start the game.
        let mut g = self.lock();
        g.busy = false;
        if self.closed.load(Ordering::SeqCst) {
            drop(g);
            for (_, _, p) in started {
                p.quit();
            }
            return Err(tr("セッションは閉じられました").into());
        }
        let start = match &cfg.start_sfen {
            Some(s) => {
                g.record = GameRecord::from_sfen(s).map_err(|e| kifu_error(&e))?;
                0
            }
            None => {
                let c = g.cursor;
                if g.record.nodes[c].special.is_some() { g.record.nodes[c].parent.unwrap_or(0) } else { c }
            }
        };
        let mut players: [Player; 2] = [0, 1].map(|s| {
            let p = &cfg.players[s];
            Player {
                kind: p.kind.clone(),
                name: p.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| tr("あなた").into()),
                key: None,
                nodes: p.nodes.filter(|&n| n > 0),
                depth: p.depth.filter(|&d| d > 0),
            }
        });
        for (side, name, proc) in started {
            let role = if side == 0 { Role::Black } else { Role::White };
            let key = self.install_engine(&mut g, proc, name.clone(), role);
            players[side].key = Some(key);
            players[side].name = name;
        }
        let (bk, wk) = g.record.side_names();
        let (bn, wn) = (players[0].name.clone(), players[1].name.clone());
        g.record.set_header(bk, &bn);
        g.record.set_header(wk, &wn);
        g.record.set_header("開始日時", &now_string());
        g.cursor = start;
        g.game = Some(Game {
            remaining: [cfg.times[0].main_ms, cfg.times[1].main_ms],
            players,
            times: cfg.times,
            turn_started: Instant::now(),
            tip: start,
            over: false,
            result: None,
            max_moves: cfg.max_moves,
        });
        g.status = tr("対局中").into();
        let board = *g.board_at(start);
        if board.legal_moves().is_empty() {
            g.finish(Special::Checkmate);
        } else {
            g.advance();
        }
        self.publish(&mut g);
        Ok(())
    }

    pub fn stop_game(&self) -> Result<(), String> {
        self.with(|g| {
            if !g.game_active() {
                return Err(tr("対局中ではありません").into());
            }
            g.finish(Special::Interrupt);
            Ok(())
        })
    }

    pub fn resign(&self) -> Result<(), String> {
        self.with(|g| {
            let Some(game) = g.game.as_ref().filter(|x| !x.over) else {
                return Err(tr("対局中ではありません").into());
            };
            let side = g.board_at(game.tip).side as usize;
            if game.players[side].key.is_some() {
                return Err(tr("投了は自分の手番で行ってください").into());
            }
            g.finish(Special::Resign);
            Ok(())
        })
    }

    /// Ask the engine to move now (`stop`); its `bestmove` is accepted.
    pub fn move_now(&self) -> Result<(), String> {
        self.with(|g| {
            let Some(game) = g.game.as_ref().filter(|x| !x.over) else {
                return Err(tr("対局中ではありません").into());
            };
            let side = g.board_at(game.tip).side as usize;
            let key = game.players[side].key.ok_or(tr("人間の手番です"))?;
            g.send(key, "stop");
            Ok(())
        })
    }

    pub fn start_analysis(
        self: &Arc<Self>,
        registry: &Registry,
        engine_id: &str,
        multipv: u32,
        batch_ms: Option<u64>,
    ) -> Result<(), String> {
        let ec = registry.get(engine_id).ok_or(tr("登録されていないエンジンです"))?;
        let multipv = multipv.clamp(1, 20);
        self.with(|g| {
            if g.busy {
                return Err(tr("エンジン起動中です").into());
            }
            if g.game_active() {
                return Err(tr("対局中は検討できません").into());
            }
            g.stop_analysis();
            g.busy = true;
            g.status = trf("{name} を起動しています…", &[("name", &ec.name)]);
            Ok(())
        })?;
        let overrides = [("MultiPV".to_owned(), multipv.to_string())];
        let proc = match start_engine(Path::new(&ec.path), &ec.effective_values(&overrides), READY_TIMEOUT) {
            Ok((p, _)) => p,
            Err(e) => {
                let msg = trf("{name} を起動できません: {err}", &[("name", &ec.name), ("err", &e)]);
                return self.with(|g| {
                    g.busy = false;
                    g.status = msg.clone();
                    Err(msg.clone())
                });
            }
        };
        let mut g = self.lock();
        g.busy = false;
        if self.closed.load(Ordering::SeqCst) {
            drop(g);
            proc.quit();
            return Err(tr("セッションは閉じられました").into());
        }
        let key = self.install_engine(&mut g, proc, ec.name.clone(), Role::Analysis);
        let node = if batch_ms.is_some() { 0 } else { g.cursor };
        g.analysis = Some(Analysis { key, engine: ec.name.clone(), multipv, node, batch_ms });
        g.status = if batch_ms.is_some() { tr("棋譜解析中…").into() } else { trf("検討中: {name}", &[("name", &ec.name)]) };
        g.cursor = node;
        g.analyze_at(node);
        self.publish(&mut g);
        Ok(())
    }

    pub fn stop_analysis(&self) -> Result<(), String> {
        self.with(|g| {
            g.stop_analysis();
            g.status = String::new();
            Ok(())
        })
    }

    fn editable(g: &Inner) -> Result<(), String> {
        if g.game_active() {
            return Err(tr("対局中は変更できません").into());
        }
        if g.busy {
            return Err(tr("エンジン起動中です").into());
        }
        Ok(())
    }

    pub fn load_record(&self, rec: GameRecord, title: Option<String>) -> Result<(), String> {
        self.with(|g| {
            Self::editable(g)?;
            if g.analysis.as_ref().is_some_and(|a| a.batch_ms.is_some()) {
                g.stop_analysis();
            }
            g.record = rec;
            g.cursor = 0;
            g.game = None;
            if let Some(t) = title {
                g.title = t;
            }
            g.status = String::new();
            if g.analysis.is_some() {
                let c = g.cursor;
                g.analyze_at(c);
            }
            Ok(())
        })
    }

    /// Paste a kifu or position. USI/SFEN positions open at their last move.
    pub fn import_text(&self, text: &str, format: Option<Format>) -> Result<(), String> {
        if text.trim_start().starts_with("SPG-TEXT ") {
            let data = spg::text::parse(text.trim_start()).map_err(crate::spgio::spg_error)?;
            let (mut recs, _, errors) = sakiyomi_spg::spg_to_records(&data);
            if recs.is_empty() {
                return Err(errors.first().cloned().unwrap_or_else(|| tr("棋譜が含まれていません").to_owned()));
            }
            return self.load_record(recs.remove(0), None);
        }
        let format = format.unwrap_or_else(|| format::detect(text));
        let rec = format::read(text, Some(format), &Limits::default()).map_err(|e| kifu_error(&e))?;
        self.load_record(rec, None)?;
        if format == Format::Usi {
            let end = self.lock().record.line_end(0);
            self.set_cursor(end)?;
        }
        Ok(())
    }

    /// Open a record file. SPG files may hold several records: the first is loaded here and
    /// the others are returned for the caller to open elsewhere.
    pub fn load_file(&self, path: &Path) -> Result<LoadNotes, String> {
        let title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        if crate::spgio::SpgKind::of_path(path).is_some() {
            let mut r = crate::spgio::read_records(path)?;
            let first = r.records.remove(0);
            self.load_record(first, title)?;
            return Ok(LoadNotes { more: r.records, lost: r.lost, errors: r.errors });
        }
        self.load_text_file(path, title)?;
        Ok(LoadNotes::default())
    }

    fn load_text_file(&self, path: &Path, title: Option<String>) -> Result<(), String> {
        let bytes = crate::tools::read_limited(path, Limits::default().max_text_bytes)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let fmt = Format::from_name(ext);
        let rec = format::read(&format::decode(&bytes), fmt, &Limits::default()).map_err(|e| kifu_error(&e))?;
        self.load_record(rec, title)
    }

    /// Save as SPG, the standard format (binary, or the `.spgt` text form).
    pub fn save_spg(&self, path: &Path, text: bool) -> Result<(), String> {
        let bytes = {
            let g = self.lock();
            crate::spgio::records_to_bytes(std::slice::from_ref(&g.record), text)?
        };
        crate::tools::atomic_write(path, &bytes, true)
    }

    pub fn export(&self, what: &str, eval_style: Option<EvalStyle>) -> Result<String, String> {
        let g = self.lock();
        match what {
            "sfen" => Ok(format::sfen_at(&g.record, g.cursor)),
            "usi" => Ok(format::usi_position(&g.record, g.cursor)),
            "bod" => {
                let names = g.record.side_names();
                Ok(sakiyomi_kifu::kif::bod(&g.record.nodes[g.cursor].board, names))
            }
            "spgt" => {
                let bytes = crate::spgio::records_to_bytes(std::slice::from_ref(&g.record), true)?;
                String::from_utf8(bytes).map_err(|e| e.to_string())
            }
            other => {
                let f = Format::from_name(other).ok_or(tr("未対応の形式です"))?;
                format::write(&g.record, f, TextEncoding::Utf8, eval_style).map_err(|e| kifu_error(&e))
            }
        }
    }

    pub fn save_file(&self, path: &Path, fmt: Format, encoding: TextEncoding, eval_style: Option<EvalStyle>) -> Result<(), String> {
        let bytes = {
            let g = self.lock();
            let text = format::write(&g.record, fmt, encoding, eval_style).map_err(|e| kifu_error(&e))?;
            format::encode(&text, crate::tools::encoding_for(path, fmt, encoding))
        };
        crate::tools::atomic_write(path, &bytes, true)
    }

    pub fn set_position(&self, sfen: &str) -> Result<(), String> {
        let rec = GameRecord::from_sfen(sfen).map_err(|e| kifu_error(&e))?;
        self.load_record(rec, None)
    }

    pub fn delete_node(&self, node: usize) -> Result<(), String> {
        self.with(|g| {
            Self::editable(g)?;
            if node == 0 || node >= g.record.nodes.len() {
                return Err(tr("開始局面は削除できません").into());
            }
            if g.analysis.as_ref().is_some_and(|a| a.batch_ms.is_some()) {
                return Err(tr("棋譜解析中は削除できません").into());
            }
            let analysis_key = g.analysis.as_ref().map(|a| a.key);
            let (parent, map) = g.record.remove_subtree(node).ok_or(tr("削除できません"))?;
            g.cursor = map[g.cursor].unwrap_or(parent);
            if let Some(game) = g.game.as_mut() {
                game.tip = map[game.tip].unwrap_or(parent);
            }
            for s in g.slots.iter_mut() {
                s.search_node = map[s.search_node].unwrap_or(parent);
            }
            if let Some(a) = g.analysis.as_mut() {
                a.node = map[a.node].unwrap_or(usize::MAX);
            }
            if analysis_key.is_some() {
                g.cursor_changed();
            }
            Ok(())
        })
    }

    pub fn promote_variation(&self, node: usize) -> Result<(), String> {
        self.with(|g| {
            if node >= g.record.nodes.len() {
                return Err(tr("存在しない局面です").into());
            }
            g.record.promote_variation(node);
            Ok(())
        })
    }

    pub fn set_comment(&self, node: usize, text: &str) -> Result<(), String> {
        self.with(|g| {
            let n = g.record.nodes.get_mut(node).ok_or(tr("存在しない局面です"))?;
            n.comment = text.replace("\r\n", "\n");
            Ok(())
        })
    }

    pub fn set_header(&self, key: &str, value: &str) -> Result<(), String> {
        self.with(|g| {
            g.record.set_header(key, value);
            Ok(())
        })
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut g = self.lock();
        let keys: Vec<u64> = g.slots.iter().map(|s| s.key).collect();
        for k in keys {
            g.retire(k, None);
        }
        g.analysis = None;
        if let Some(game) = g.game.as_mut() {
            game.over = true;
        }
        let dead = std::mem::take(&mut g.graveyard);
        drop(g);
        for p in dead {
            p.quit();
        }
    }
}

// ------------------------------------------------------------------ manager

pub struct Manager {
    sessions: Mutex<BTreeMap<u32, Arc<Session>>>,
    next: AtomicU32,
    sink: Sink,
}

impl Manager {
    pub fn new(sink: Sink) -> Self {
        Manager { sessions: Mutex::new(BTreeMap::new()), next: AtomicU32::new(1), sink }
    }

    pub fn create(&self, title: Option<String>) -> Arc<Session> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let inner = Inner {
            id,
            title: title.unwrap_or_default(),
            record: GameRecord::from_start(),
            cursor: 0,
            game: None,
            analysis: None,
            slots: Vec::new(),
            graveyard: Vec::new(),
            status: String::new(),
            busy: false,
            dirty_thinking: BTreeSet::new(),
            next_key: 0,
        };
        let session = Arc::new(Session { id, inner: Mutex::new(inner), sink: self.sink.clone(), closed: AtomicBool::new(false) });
        let weak = Arc::downgrade(&session);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(120));
            let Some(s) = weak.upgrade() else { return };
            if s.closed.load(Ordering::SeqCst) {
                return;
            }
            s.tick();
        });
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(id, session.clone());
        (self.sink)(Event::Session(session.view()));
        session
    }

    pub fn get(&self, id: u32) -> Result<Arc<Session>, String> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().ok_or_else(|| trf("セッション {id} がありません", &[("id", &id)]))
    }

    pub fn list(&self) -> Vec<SessionView> {
        let all: Vec<Arc<Session>> = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        all.iter().map(|s| s.view()).collect()
    }

    /// (session id, engine name, process id) across all sessions.
    pub fn engine_processes(&self) -> Vec<(u32, String, u32)> {
        let all: Vec<Arc<Session>> = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        all.iter()
            .flat_map(|s| s.engine_processes().into_iter().map(move |(n, p)| (s.id, n, p)))
            .collect()
    }

    pub fn close(&self, id: u32) {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if let Some(s) = s {
            s.close();
            (self.sink)(Event::Closed { id });
        }
    }

    pub fn close_all(&self) {
        let all: Vec<Arc<Session>> = std::mem::take(&mut *self.sessions.lock().unwrap_or_else(|e| e.into_inner())).into_values().collect();
        for s in all {
            s.close();
        }
    }
}
