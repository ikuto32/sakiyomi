//! Game and position databases used for training and research:
//!
//! | format | unit | layout |
//! |---|---|---|
//! | `hcpe` (cshogi/dlshogi) | position | 38 B: hcp, i16 eval (side to move), u16 cshogi move16, u8 result, pad |
//! | `psv` (YaneuraOu PackedSfenValue) | position | 40 B: psfen, i16 score (side to move), u16 YaneuraOu move16, u16 ply, i8 result (side to move), pad |
//! | `hcpe3` (dlshogi) | game | 36 B header (hcp, u16 moves, u8 result, u8 info) + per move (u16 move16, i16 eval, u16 n, n × (u16 move16, u16 visits)) |
//! | SFEN lines | game | one `position …`/`startpos …`/SFEN line per game |
//! | CSA multi | game | CSA records separated by a line containing only `/` |
//!
//! Readers stream from any `Read`, never trust length fields beyond fixed caps, and report a
//! truncated trailing record as an error.

use std::io::{self, BufRead, Read, Write};

use shogi_core::Move16;
use shogi_core::rules::Board;

use crate::error::{Error, ErrorKind, Limits, Result};
use crate::packed::{self, PACKED_LEN};
use crate::record::{Eval, GameRecord, Special};
use crate::{csa, format};

/// dlshogi caps (HCPE3_MAX_MOVE_NUM and max legal moves).
const MAX_HCPE3_MOVES: usize = 513;
const MAX_CANDIDATES: usize = 593;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DbFormat {
    Hcpe,
    Psv,
    Hcpe3,
    SfenLines,
    CsaMulti,
}

impl DbFormat {
    pub const ALL: [DbFormat; 5] = [DbFormat::Hcpe, DbFormat::Psv, DbFormat::Hcpe3, DbFormat::SfenLines, DbFormat::CsaMulti];

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().trim_start_matches('.') {
            "hcpe" => Some(DbFormat::Hcpe),
            "psv" | "bin" | "packedsfenvalue" => Some(DbFormat::Psv),
            "hcpe3" => Some(DbFormat::Hcpe3),
            "sfen" | "sfens" | "txt" | "usi" => Some(DbFormat::SfenLines),
            "csa" => Some(DbFormat::CsaMulti),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DbFormat::Hcpe => "hcpe",
            DbFormat::Psv => "psv",
            DbFormat::Hcpe3 => "hcpe3",
            DbFormat::SfenLines => "sfen",
            DbFormat::CsaMulti => "csa",
        }
    }

    /// Formats that store whole games (convertible to every format).
    pub fn is_game_format(self) -> bool {
        matches!(self, DbFormat::Hcpe3 | DbFormat::SfenLines | DbFormat::CsaMulti)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameResult {
    #[default]
    Unknown,
    Draw,
    BlackWin,
    WhiteWin,
}

impl GameResult {
    fn for_side(self, side: u8) -> i8 {
        match (self, side) {
            (GameResult::BlackWin, 0) | (GameResult::WhiteWin, 1) => 1,
            (GameResult::BlackWin, _) | (GameResult::WhiteWin, _) => -1,
            _ => 0,
        }
    }
    fn from_side(r: i8, side: u8) -> Self {
        match (r.signum(), side) {
            (0, _) => GameResult::Draw,
            (1, 0) | (-1, 1) => GameResult::BlackWin,
            _ => GameResult::WhiteWin,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbMove {
    pub mv: Move16,
    /// Evaluation by the side that made this move, from its own point of view.
    pub eval: Option<i32>,
    /// Search candidates (move, visits), hcpe3 only.
    pub candidates: Vec<(Move16, u16)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbGame {
    pub start: Board,
    pub moves: Vec<DbMove>,
    pub result: GameResult,
    /// hcpe3 result flags: 4 repetition, 8 entering king, 16 max moves.
    pub flags: u8,
    pub info: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbPosition {
    pub board: Board,
    /// Evaluation from the side to move's point of view.
    pub eval: i16,
    pub best: Option<Move16>,
    pub result: GameResult,
    pub ply: u16,
}

fn corrupt(m: impl Into<String>) -> Error {
    Error::new(ErrorKind::Corrupt(m.into()))
}

fn io_err(e: io::Error) -> Error {
    corrupt(format!("I/O: {e}"))
}

/// Read exactly `buf.len()` bytes; Ok(false) at a clean end of input.
fn read_record<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(corrupt(format!("truncated record ({filled} of {} bytes)", buf.len()))),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_err(e)),
        }
    }
    Ok(true)
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

// ---------------------------------------------------------------- positions

pub fn read_position<R: Read>(format: DbFormat, r: &mut R) -> Result<Option<DbPosition>> {
    match format {
        DbFormat::Hcpe => {
            let mut b = [0u8; 38];
            if !read_record(r, &mut b)? {
                return Ok(None);
            }
            let board = packed::from_hcp(b[..PACKED_LEN].try_into().map_err(|_| corrupt("hcp"))?)?;
            let result = match b[36] & 3 {
                1 => GameResult::BlackWin,
                2 => GameResult::WhiteWin,
                _ => GameResult::Draw,
            };
            let ply = board.move_number.min(u16::MAX as u32) as u16;
            Ok(Some(DbPosition {
                board,
                eval: le16(&b, 32) as i16,
                best: packed::from_cshogi_move16(le16(&b, 34)),
                result,
                ply,
            }))
        }
        DbFormat::Psv => {
            let mut b = [0u8; 40];
            if !read_record(r, &mut b)? {
                return Ok(None);
            }
            let mut board = packed::from_psfen(b[..PACKED_LEN].try_into().map_err(|_| corrupt("psfen"))?)?;
            let ply = le16(&b, 36);
            board.move_number = u32::from(ply.max(1));
            let result = GameResult::from_side(b[38] as i8, board.side);
            Ok(Some(DbPosition { board, eval: le16(&b, 32) as i16, best: packed::from_yaneuraou_move16(le16(&b, 34)), result, ply }))
        }
        other => Err(Error::new(ErrorKind::Unsupported(format!("{} stores games, not positions", other.name())))),
    }
}

pub fn write_position<W: Write>(format: DbFormat, w: &mut W, p: &DbPosition) -> Result<()> {
    match format {
        DbFormat::Hcpe => {
            let mut b = [0u8; 38];
            b[..32].copy_from_slice(&packed::to_hcp(&p.board)?);
            b[32..34].copy_from_slice(&p.eval.to_le_bytes());
            b[34..36].copy_from_slice(&p.best.map(packed::to_cshogi_move16).unwrap_or(0).to_le_bytes());
            b[36] = match p.result {
                GameResult::BlackWin => 1,
                GameResult::WhiteWin => 2,
                _ => 0,
            };
            w.write_all(&b).map_err(io_err)
        }
        DbFormat::Psv => {
            let mut b = [0u8; 40];
            b[..32].copy_from_slice(&packed::to_psfen(&p.board)?);
            b[32..34].copy_from_slice(&p.eval.to_le_bytes());
            b[34..36].copy_from_slice(&p.best.map(packed::to_yaneuraou_move16).unwrap_or(0).to_le_bytes());
            b[36..38].copy_from_slice(&p.ply.to_le_bytes());
            b[38] = p.result.for_side(p.board.side) as u8;
            w.write_all(&b).map_err(io_err)
        }
        other => Err(Error::new(ErrorKind::Unsupported(format!("{} stores games, not positions", other.name())))),
    }
}

/// Every position of a game with the move played from it.
pub fn game_positions(g: &DbGame) -> Result<Vec<DbPosition>> {
    let mut b = g.start;
    let mut out = Vec::with_capacity(g.moves.len());
    for m in &g.moves {
        if !b.legal_moves().contains(&m.mv) {
            return Err(ErrorKind::IllegalMove(m.mv.to_usi()).into());
        }
        out.push(DbPosition {
            board: b,
            eval: clamp_i16(m.eval.unwrap_or(0)),
            best: Some(m.mv),
            result: g.result,
            ply: b.move_number.min(u16::MAX as u32) as u16,
        });
        b = b.after_generated(m.mv);
    }
    Ok(out)
}

// ---------------------------------------------------------------- games

/// Line reader that refuses lines longer than the limit instead of buffering them.
fn read_bounded_line<R: BufRead>(r: &mut R, limit: usize, out: &mut String) -> Result<bool> {
    out.clear();
    let mut bytes = Vec::new();
    let n = r.take(limit as u64 + 1).read_until(b'\n', &mut bytes).map_err(io_err)?;
    if n == 0 {
        return Ok(false);
    }
    if bytes.len() > limit && !bytes.ends_with(b"\n") {
        return Err(ErrorKind::TooLarge { what: "line", limit }.into());
    }
    out.push_str(&format::decode(&bytes));
    Ok(true)
}

/// Streaming reader of games.
pub struct GameReader<R: BufRead> {
    format: DbFormat,
    r: R,
    limits: Limits,
    games: usize,
    line_no: usize,
    done: bool,
}

impl<R: BufRead> GameReader<R> {
    pub fn new(format: DbFormat, r: R, limits: Limits) -> Self {
        GameReader { format, r, limits, games: 0, line_no: 0, done: false }
    }

    fn next_game(&mut self) -> Result<Option<DbGame>> {
        if self.limits.max_games > 0 && self.games >= self.limits.max_games {
            return Ok(None);
        }
        let g = match self.format {
            DbFormat::Hcpe3 => self.read_hcpe3()?,
            DbFormat::SfenLines => self.read_sfen_line()?,
            DbFormat::CsaMulti => self.read_csa_game()?,
            other => return Err(Error::new(ErrorKind::Unsupported(format!("{} stores positions, not games", other.name())))),
        };
        if g.is_some() {
            self.games += 1;
        }
        Ok(g)
    }

    fn read_hcpe3(&mut self) -> Result<Option<DbGame>> {
        let at = self.games + 1;
        let mut h = [0u8; 36];
        if !read_record(&mut self.r, &mut h).map_err(|e| e.at(at))? {
            return Ok(None);
        }
        let start = packed::from_hcp(h[..32].try_into().map_err(|_| corrupt("hcp"))?).map_err(|e| e.at(at))?;
        let n = le16(&h, 32) as usize;
        if n > MAX_HCPE3_MOVES {
            return Err(corrupt(format!("{n} moves in one game")).at(at));
        }
        let mut moves = Vec::with_capacity(n);
        let mut b = start;
        for _ in 0..n {
            let mut m = [0u8; 6];
            if !read_record(&mut self.r, &mut m)? {
                return Err(corrupt("truncated game").at(at));
            }
            let mv = packed::from_cshogi_move16(le16(&m, 0)).ok_or_else(|| corrupt("move16").at(at))?;
            let cands = le16(&m, 4) as usize;
            if cands > MAX_CANDIDATES {
                return Err(corrupt(format!("{cands} candidates")).at(at));
            }
            let mut candidates = Vec::with_capacity(cands);
            for _ in 0..cands {
                let mut c = [0u8; 4];
                if !read_record(&mut self.r, &mut c)? {
                    return Err(corrupt("truncated candidates").at(at));
                }
                if let Some(cm) = packed::from_cshogi_move16(le16(&c, 0)) {
                    candidates.push((cm, le16(&c, 2)));
                }
            }
            if !b.legal_moves().contains(&mv) {
                return Err(Error::new(ErrorKind::IllegalMove(mv.to_usi())).at(at));
            }
            b = b.after_generated(mv);
            moves.push(DbMove { mv, eval: Some(i32::from(le16(&m, 2) as i16)), candidates });
        }
        let result = match h[34] & 3 {
            1 => GameResult::BlackWin,
            2 => GameResult::WhiteWin,
            _ => GameResult::Draw,
        };
        Ok(Some(DbGame { start, moves, result, flags: h[34] & !3, info: h[35] }))
    }

    fn read_sfen_line(&mut self) -> Result<Option<DbGame>> {
        let mut line = String::new();
        loop {
            if !read_bounded_line(&mut self.r, self.limits.max_line_bytes, &mut line)? {
                return Ok(None);
            }
            self.line_no += 1;
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let rec = format::read_usi(t, &self.limits).map_err(|e| e.at(self.line_no))?;
            return Ok(Some(record_to_game(&rec)));
        }
    }

    fn read_csa_game(&mut self) -> Result<Option<DbGame>> {
        let mut text = String::new();
        let mut line = String::new();
        let start_line = self.line_no + 1;
        loop {
            let more = read_bounded_line(&mut self.r, self.limits.max_line_bytes, &mut line)?;
            if more {
                self.line_no += 1;
            }
            if !more || line.trim() == "/" {
                if text.trim().is_empty() {
                    if !more {
                        return Ok(None);
                    }
                    continue;
                }
                let rec = csa::read_csa(&text, &self.limits).map_err(|e| {
                    let l = e.line.map(|l| l + start_line - 1);
                    Error { kind: e.kind, line: l }
                })?;
                return Ok(Some(record_to_game(&rec)));
            }
            if text.len() + line.len() > self.limits.max_text_bytes {
                return Err(ErrorKind::TooLarge { what: "game", limit: self.limits.max_text_bytes }.into());
            }
            text.push_str(&line);
        }
    }
}

impl<R: BufRead> Iterator for GameReader<R> {
    type Item = Result<DbGame>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.next_game() {
            Ok(Some(g)) => Some(Ok(g)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

pub fn write_game<W: Write>(format: DbFormat, w: &mut W, g: &DbGame) -> Result<()> {
    match format {
        DbFormat::Hcpe3 => {
            if g.moves.len() > MAX_HCPE3_MOVES {
                return Err(ErrorKind::TooLarge { what: "hcpe3 moves", limit: MAX_HCPE3_MOVES }.into());
            }
            let mut h = [0u8; 36];
            h[..32].copy_from_slice(&packed::to_hcp(&g.start)?);
            h[32..34].copy_from_slice(&(g.moves.len() as u16).to_le_bytes());
            h[34] = g.flags & !3
                | match g.result {
                    GameResult::BlackWin => 1,
                    GameResult::WhiteWin => 2,
                    _ => 0,
                };
            h[35] = g.info;
            w.write_all(&h).map_err(io_err)?;
            for m in &g.moves {
                let n = m.candidates.len().min(MAX_CANDIDATES);
                let mut b = Vec::with_capacity(6 + n * 4);
                b.extend_from_slice(&packed::to_cshogi_move16(m.mv).to_le_bytes());
                b.extend_from_slice(&clamp_i16(m.eval.unwrap_or(0)).to_le_bytes());
                b.extend_from_slice(&(n as u16).to_le_bytes());
                for (cm, v) in m.candidates.iter().take(n) {
                    b.extend_from_slice(&packed::to_cshogi_move16(*cm).to_le_bytes());
                    b.extend_from_slice(&v.to_le_bytes());
                }
                w.write_all(&b).map_err(io_err)?;
            }
            Ok(())
        }
        DbFormat::SfenLines => {
            let moves: Vec<Move16> = g.moves.iter().map(|m| m.mv).collect();
            let mut start = g.start;
            start.move_number = start.move_number.max(1);
            let line = format::usi_position_from(&start.to_sfen(), &moves);
            writeln!(w, "{}", line.trim_start_matches("position ")).map_err(io_err)
        }
        DbFormat::CsaMulti => {
            let rec = game_to_record(g)?;
            w.write_all(csa::write_csa(&rec, Some(crate::eval_comment::EvalStyle::Floodgate)).as_bytes()).map_err(io_err)?;
            writeln!(w, "/").map_err(io_err)
        }
        other => Err(Error::new(ErrorKind::Unsupported(format!("{} stores positions, not games", other.name())))),
    }
}

/// Main line of a record as a database game (evaluations from the movers' readings).
pub fn record_to_game(rec: &GameRecord) -> DbGame {
    let line = rec.mainline();
    let mut moves = Vec::new();
    let mut result = GameResult::Unknown;
    let mut flags = 0;
    for &n in line.iter().skip(1) {
        let node = &rec.nodes[n];
        if let Some(sp) = node.special {
            result = match sp.winner(node.board.side) {
                Some(0) => GameResult::BlackWin,
                Some(_) => GameResult::WhiteWin,
                None if matches!(sp, Special::Sennichite | Special::Jishogi | Special::Draw | Special::MaxMoves) => GameResult::Draw,
                None => GameResult::Unknown,
            };
            flags = match sp {
                Special::Sennichite => 4,
                Special::EnteringKingWin => 8,
                Special::Jishogi => 16,
                Special::MaxMoves => 16,
                _ => 0,
            };
            break;
        }
        let Some(mv) = node.mv else { continue };
        let mover = 1 - node.board.side;
        let reading = if mover == 0 { &node.analysis.black } else { &node.analysis.white };
        let eval = reading.as_ref().and_then(|r| r.eval).map(|e: Eval| e.for_side(mover).0);
        moves.push(DbMove { mv, eval, candidates: Vec::new() });
    }
    DbGame { start: *rec.initial(), moves, result, flags, info: 0 }
}

pub fn game_to_record(g: &DbGame) -> Result<GameRecord> {
    let mut rec = GameRecord::new(g.start);
    let mut cur = 0;
    for m in &g.moves {
        let mover = rec.nodes[cur].board.side;
        cur = rec.add_move(cur, m.mv)?;
        if let Some(e) = m.eval {
            *rec.nodes[cur].analysis.slot_mut(mover) = Some(crate::record::EngineEval {
                eval: Some(Eval::from_side_to_move(e, None, mover)),
                ..Default::default()
            });
        }
    }
    let side = rec.nodes[cur].board.side;
    let special = match g.result {
        GameResult::BlackWin | GameResult::WhiteWin => {
            let winner = if g.result == GameResult::BlackWin { 0 } else { 1 };
            if g.flags & 8 != 0 {
                Some(Special::EnteringKingWin)
            } else if winner == side {
                Some(Special::IllegalWin)
            } else {
                Some(Special::Resign)
            }
        }
        GameResult::Draw if g.flags & 4 != 0 => Some(Special::Sennichite),
        GameResult::Draw => Some(Special::Jishogi),
        GameResult::Unknown => None,
    };
    if let Some(sp) = special {
        rec.add_special(cur, sp);
    }
    Ok(rec)
}

// ---------------------------------------------------------------- conversion

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertStats {
    pub games: u64,
    pub positions_written: u64,
    pub skipped: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConvertOptions {
    /// When writing position formats from games, skip positions whose move has no evaluation.
    pub skip_without_eval: bool,
    /// Skip positions before this ply (0 = none), e.g. to drop opening-book moves.
    pub min_ply: u16,
}

/// Convert between database formats, streaming. `tick` is called per game/position and may
/// return `false` to cancel (the output written so far is then incomplete).
pub fn convert<R: BufRead, W: Write>(
    from: DbFormat,
    input: R,
    to: DbFormat,
    output: &mut W,
    opts: &ConvertOptions,
    limits: &Limits,
    mut tick: impl FnMut(&ConvertStats) -> bool,
) -> Result<ConvertStats> {
    let mut stats = ConvertStats::default();
    if from.is_game_format() {
        for g in GameReader::new(from, input, *limits) {
            let g = g?;
            stats.games += 1;
            if to.is_game_format() {
                write_game(to, output, &g)?;
            } else {
                for (p, m) in game_positions(&g)?.iter().zip(&g.moves) {
                    emit_position(to, output, p, m.eval.is_some(), opts, &mut stats)?;
                }
            }
            if !tick(&stats) {
                break;
            }
        }
    } else {
        if to.is_game_format() {
            return Err(Error::new(ErrorKind::Unsupported(format!(
                "{} holds single positions; it cannot be converted to games ({})",
                from.name(),
                to.name()
            ))));
        }
        let mut input = input;
        let mut index = 0usize;
        loop {
            index += 1;
            let Some(p) = read_position(from, &mut input).map_err(|e| e.at(index))? else { break };
            emit_position(to, output, &p, true, opts, &mut stats)?;
            if !tick(&stats) {
                break;
            }
        }
    }
    Ok(stats)
}

/// Write one position of a game unless the options skip it.
pub fn emit_position<W: Write>(
    to: DbFormat,
    output: &mut W,
    p: &DbPosition,
    has_eval: bool,
    opts: &ConvertOptions,
    stats: &mut ConvertStats,
) -> Result<()> {
    if (opts.skip_without_eval && !has_eval) || p.ply < opts.min_ply {
        stats.skipped += 1;
        return Ok(());
    }
    write_position(to, output, p)?;
    stats.positions_written += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_games() -> Vec<DbGame> {
        let l = Limits::default();
        let mut a = record_to_game(&format::read_usi("position startpos moves 7g7f 3c3d 8h2b+ 3a2b B*4e", &l).unwrap());
        a.result = GameResult::BlackWin;
        for (i, m) in a.moves.iter_mut().enumerate() {
            m.eval = Some(10 * i as i32 - 20);
        }
        a.moves[2].candidates = vec![(Move16::from_usi("8h2b+").unwrap(), 120), (Move16::from_usi("2g2f").unwrap(), 30)];
        let b = record_to_game(&format::read_usi("sfen lnsgkgsnl/1r7/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1 moves 3c3d", &l).unwrap());
        vec![a, b]
    }

    #[test]
    fn hcpe3_roundtrip() {
        let games = sample_games();
        let mut buf = Vec::new();
        for g in &games {
            write_game(DbFormat::Hcpe3, &mut buf, g).unwrap();
        }
        let back: Vec<DbGame> = GameReader::new(DbFormat::Hcpe3, buf.as_slice(), Limits::default()).map(|g| g.unwrap()).collect();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].moves, games[0].moves);
        assert_eq!(back[0].result, GameResult::BlackWin);
        assert_eq!(back[1].start.key(), games[1].start.key());
        // Truncation is an error, not silent data loss.
        let cut = &buf[..buf.len() - 3];
        assert!(GameReader::new(DbFormat::Hcpe3, cut, Limits::default()).any(|g| g.is_err()));
    }

    #[test]
    fn positions_roundtrip_hcpe_and_psv() {
        let games = sample_games();
        let positions = game_positions(&games[0]).unwrap();
        for f in [DbFormat::Hcpe, DbFormat::Psv] {
            let mut buf = Vec::new();
            for p in &positions {
                write_position(f, &mut buf, p).unwrap();
            }
            assert_eq!(buf.len(), positions.len() * if f == DbFormat::Hcpe { 38 } else { 40 });
            let mut r = buf.as_slice();
            let mut back = Vec::new();
            while let Some(p) = read_position(f, &mut r).unwrap() {
                back.push(p);
            }
            for (a, b) in back.iter().zip(&positions) {
                assert_eq!(a.board.key(), b.board.key());
                assert_eq!(a.eval, b.eval);
                assert_eq!(a.best, b.best);
                assert_eq!(a.result, b.result, "{f:?}");
            }
            // A truncated final record is an error, not silently dropped.
            let mut cut = &buf[..buf.len() - 1];
            let mut saw_err = false;
            loop {
                match read_position(f, &mut cut) {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        saw_err = true;
                        break;
                    }
                }
            }
            assert!(saw_err, "{f:?}");
        }
    }

    #[test]
    fn text_game_formats_and_conversion() {
        let games = sample_games();
        for f in [DbFormat::SfenLines, DbFormat::CsaMulti] {
            let mut buf = Vec::new();
            for g in &games {
                write_game(f, &mut buf, g).unwrap();
            }
            let back: Vec<DbGame> = GameReader::new(f, buf.as_slice(), Limits::default()).map(|g| g.unwrap()).collect();
            assert_eq!(back.len(), 2, "{f:?}");
            assert_eq!(back[0].moves.iter().map(|m| m.mv).collect::<Vec<_>>(), games[0].moves.iter().map(|m| m.mv).collect::<Vec<_>>());
        }
        // games → hcpe → psv
        let mut hcpe3 = Vec::new();
        for g in &games {
            write_game(DbFormat::Hcpe3, &mut hcpe3, g).unwrap();
        }
        let mut hcpe = Vec::new();
        let s = convert(DbFormat::Hcpe3, hcpe3.as_slice(), DbFormat::Hcpe, &mut hcpe, &ConvertOptions::default(), &Limits::default(), |_| true).unwrap();
        assert_eq!(s.games, 2);
        assert_eq!(s.positions_written, 6);
        let mut psv = Vec::new();
        let s = convert(DbFormat::Hcpe, hcpe.as_slice(), DbFormat::Psv, &mut psv, &ConvertOptions::default(), &Limits::default(), |_| true).unwrap();
        assert_eq!(s.positions_written, 6);
        assert!(convert(DbFormat::Psv, psv.as_slice(), DbFormat::Hcpe3, &mut Vec::new(), &ConvertOptions::default(), &Limits::default(), |_| true).is_err());
    }

    #[test]
    fn hostile_binaries() {
        let mut x = 0x2545F4914F6CDD1Du64;
        for len in [0usize, 1, 35, 36, 37, 38, 40, 100, 1000] {
            let mut d = vec![0u8; len];
            for b in d.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x as u8;
            }
            let _ = GameReader::new(DbFormat::Hcpe3, d.as_slice(), Limits::default()).count();
            let _ = read_position(DbFormat::Hcpe, &mut d.as_slice());
            let _ = read_position(DbFormat::Psv, &mut d.as_slice());
            let _ = GameReader::new(DbFormat::SfenLines, d.as_slice(), Limits::default()).count();
            let _ = GameReader::new(DbFormat::CsaMulti, d.as_slice(), Limits::default()).count();
        }
        // A line far beyond the limit is rejected without buffering it all.
        let long = vec![b'a'; 3 << 20];
        assert!(GameReader::new(DbFormat::SfenLines, long.as_slice(), Limits::default()).next().unwrap().is_err());
    }
}
