//! KIF / KI2 reading and writing (Kifu for Windows compatible).
//!
//! Readers keep comment lines verbatim; engine evaluations inside comments are recognised
//! afterwards by [`crate::eval_comment`]. Writers are iterative, so deeply nested variations
//! from untrusted files cannot exhaust the stack.

use shogi_core::rules::Board;

use crate::error::{Error, ErrorKind, Limits, Result, check_text};
use crate::eval_comment::{self, EvalStyle};
use crate::notation::{self, HAND_KINDS, kanji_number, parse_kanji_number, parse_piece_prefix, piece_char};
use crate::record::{GameRecord, Special, board_from_parts, handicap_name, handicap_sfen, parse_sfen};

/// Options for writing KIF/KI2.
#[derive(Clone, Copy, Debug)]
pub struct WriteOptions {
    /// Value of the `#KIF version=2.0 encoding=…` line (KIF only).
    pub encoding_label: &'static str,
    /// How engine evaluations are written as comments; `None` drops them.
    pub eval_style: Option<EvalStyle>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { encoding_label: "UTF-8", eval_style: Some(EvalStyle::ShogiGui) }
    }
}

/// Header and BOD state shared by the KIF and KI2 readers.
#[derive(Default)]
struct HeaderState {
    headers: Vec<(String, String)>,
    handicap: Option<String>,
    bod_rows: Vec<String>,
    hands: [Option<String>; 2],
    white_to_move: Option<bool>,
    root_comment: Vec<String>,
}

impl HeaderState {
    /// Returns true if the line was consumed as header/BOD content.
    fn take(&mut self, line: &str) -> bool {
        let t = line.trim_end();
        if t.starts_with('|') {
            self.bod_rows.push(t.to_owned());
            return true;
        }
        if t.contains("+---") || t.trim_start().starts_with("９ ８") || t.trim_start().starts_with("9 8") {
            return true;
        }
        if t == "後手番" || t == "上手番" {
            self.white_to_move = Some(true);
            return true;
        }
        if t == "先手番" || t == "下手番" {
            self.white_to_move = Some(false);
            return true;
        }
        let Some((key, value)) = t.split_once('：').or_else(|| t.split_once(':')) else {
            return false;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('\u{3000}');
        match key {
            "先手の持駒" | "下手の持駒" => self.hands[0] = Some(value.to_owned()),
            "後手の持駒" | "上手の持駒" => self.hands[1] = Some(value.to_owned()),
            "手合割" => self.handicap = Some(value.to_owned()),
            "変化" => return false,
            _ if key.is_empty() || key.chars().next().is_some_and(|c| c.is_ascii_digit()) => return false,
            _ => self.headers.push((key.to_owned(), value.to_owned())),
        }
        true
    }

    fn board(&self) -> Result<Board> {
        if self.bod_rows.len() >= 9 {
            return parse_bod(&self.bod_rows, &self.hands, self.white_to_move.unwrap_or(false));
        }
        match &self.handicap {
            Some(name) => {
                let sfen = handicap_sfen(name).ok_or_else(|| Error::new(ErrorKind::UnsupportedHandicap(name.clone())))?;
                parse_sfen(sfen)
            }
            None => parse_sfen(shogi_core::START_SFEN),
        }
    }

    fn into_record(self) -> Result<GameRecord> {
        let mut rec = GameRecord::new(self.board()?);
        rec.headers = self.headers;
        if let Some(h) = &self.handicap {
            rec.headers.insert(0, ("手合割".to_owned(), h.clone()));
        }
        rec.nodes[0].comment = self.root_comment.join("\n");
        Ok(rec)
    }
}

fn parse_hand(text: &str) -> Result<[u8; 7]> {
    let mut hand = [0u8; 7];
    if text.is_empty() || text == "なし" {
        return Ok(hand);
    }
    for token in text.split(|c: char| c.is_whitespace() || c == '、').filter(|t| !t.is_empty()) {
        let bad = || Error::new(ErrorKind::InvalidHand(token.to_owned()));
        let (kind, used) = parse_piece_prefix(token).ok_or_else(bad)?;
        let n = parse_kanji_number(&token[used..]).filter(|&n| n <= 18).ok_or_else(bad)?;
        let idx = notation::hand_index(kind).ok_or_else(bad)?;
        hand[idx] = n as u8;
    }
    Ok(hand)
}

pub(crate) fn parse_bod(rows: &[String], hands: &[Option<String>; 2], white: bool) -> Result<Board> {
    let mut squares = [0u8; 81];
    for (r, row) in rows.iter().take(9).enumerate() {
        let body: Vec<char> = row.trim_start_matches('|').chars().collect();
        let short = || Error::new(ErrorKind::InvalidBoardDiagram(row.clone()));
        for f in 0..9 {
            let prefix = *body.get(f * 2).ok_or_else(short)?;
            let piece = *body.get(f * 2 + 1).ok_or_else(short)?;
            if piece == '・' {
                continue;
            }
            let mut buf = [0u8; 4];
            let (kind, _) = parse_piece_prefix(piece.encode_utf8(&mut buf))
                .ok_or_else(|| Error::new(ErrorKind::InvalidBoardDiagram(piece.to_string())))?;
            let color = if prefix == 'v' || prefix == 'V' { 1u8 } else { 0 };
            squares[notation::square(9 - f, r + 1)] = kind | color << 4;
        }
    }
    let black = parse_hand(hands[0].as_deref().unwrap_or(""))?;
    let white_hand = parse_hand(hands[1].as_deref().unwrap_or(""))?;
    board_from_parts(squares, [black, white_hand], u8::from(white))
}

/// Parses "( 0:01/00:00:01)" style time; returns milliseconds spent on the move.
fn parse_time(text: &str) -> Option<u64> {
    let inner = text.trim().strip_prefix('(')?;
    let first = inner.split('/').next()?.trim();
    let mut parts = first.split(':');
    let m: u64 = parts.next()?.trim().parse().ok()?;
    let s: u64 = parts.next()?.trim().parse().ok()?;
    m.checked_mul(60)?.checked_add(s)?.checked_mul(1000)
}

/// Parse a "変化：N手" line.
fn variation_ply(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix("変化")?.trim_start_matches(['：', ':']).trim();
    rest.trim_end_matches('手').trim().parse().ok()
}

/// Tracks the current line while reading moves, for 変化 handling.
struct Cursor {
    /// path[k] = node at ply k on the most recently read line (never empty).
    path: Vec<usize>,
}

impl Cursor {
    fn current(&self) -> usize {
        self.path.last().copied().unwrap_or(0)
    }
    fn branch(&mut self, ply: u32) -> Result<()> {
        let keep = ply as usize;
        if keep == 0 || keep > self.path.len() {
            return Err(ErrorKind::InvalidVariation(ply).into());
        }
        self.path.truncate(keep);
        Ok(())
    }
}

fn check_nodes(rec: &GameRecord, limits: &Limits) -> Result<()> {
    if rec.nodes.len() >= limits.max_nodes {
        return Err(ErrorKind::TooLarge { what: "moves", limit: limits.max_nodes }.into());
    }
    Ok(())
}

fn append_comment(rec: &mut GameRecord, node: usize, text: &str) {
    let n = &mut rec.nodes[node];
    if !n.comment.is_empty() {
        n.comment.push('\n');
    }
    n.comment.push_str(text);
}

pub fn read_kif(text: &str, limits: &Limits) -> Result<GameRecord> {
    check_text(text, limits)?;
    let mut hs = HeaderState::default();
    let mut lines = text.lines().enumerate().peekable();
    // Header section: until the first move line or the 手数 separator.
    while let Some(&(_, line)) = lines.peek() {
        let t = line.trim();
        if t.starts_with("手数") {
            lines.next();
            break;
        }
        if is_move_line(t) {
            break;
        }
        lines.next();
        if t.is_empty() || t.starts_with('#') || t.starts_with('&') {
            continue;
        }
        if let Some(c) = t.strip_prefix('*') {
            hs.root_comment.push(c.to_owned());
            continue;
        }
        hs.take(line);
    }
    let mut rec = hs.into_record()?;
    let mut cur = Cursor { path: vec![0] };
    for (no, line) in lines {
        let t = line.trim();
        let at = |e: Error| e.at(no + 1);
        if t.is_empty() || t.starts_with('#') || t.starts_with('&') || t.starts_with("まで") {
            continue;
        }
        if let Some(c) = t.strip_prefix('*') {
            append_comment(&mut rec, cur.current(), c);
            continue;
        }
        if let Some(ply) = variation_ply(t) {
            cur.branch(ply).map_err(at)?;
            continue;
        }
        if !is_move_line(t) {
            continue;
        }
        check_nodes(&rec, limits).map_err(at)?;
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        let mut rest = t[digits..].trim_start();
        let mut move_text = String::new();
        if let Some(r) = rest.strip_prefix('同') {
            move_text.push('同');
            rest = r.trim_start();
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        move_text.push_str(rest[..end].trim_end_matches('+'));
        let time = parse_time(&rest[end..]);
        let parent = cur.current();
        if rec.nodes[parent].special.is_some() {
            continue;
        }
        let node = if let Some(sp) = Special::from_kif(&move_text) {
            rec.add_special(parent, sp)
        } else {
            let mv = notation::parse_jp_move(&rec.nodes[parent].board, &move_text, rec.prev_to(parent)).map_err(at)?;
            rec.add_move(parent, mv).map_err(at)?
        };
        rec.nodes[node].time_ms = time;
        cur.path.push(node);
    }
    eval_comment::extract_all(&mut rec);
    Ok(rec)
}

fn is_move_line(t: &str) -> bool {
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && t[digits..].starts_with(|c: char| c.is_whitespace())
}

fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn fmt_time(ms: u64, total_ms: u64) -> String {
    let s = ms / 1000;
    let t = total_ms / 1000;
    format!("({:>2}:{:02}/{:02}:{:02}:{:02})", s / 60, s % 60, t / 3600, t / 60 % 60, t % 60)
}

fn write_headers(rec: &GameRecord, out: &mut String) {
    for (k, v) in &rec.headers {
        if k != "手合割" {
            out.push_str(&format!("{k}：{v}\n"));
        }
    }
    match handicap_name(rec.initial()) {
        Some(name) if rec.initial().side == if name == "平手" { 0 } else { 1 } => {
            out.push_str(&format!("手合割：{name}\n"));
        }
        _ => out.push_str(&bod(rec.initial(), rec.side_names())),
    }
}

fn hand_text(hand: &[u8; 7]) -> String {
    let parts: Vec<String> = [6usize, 5, 4, 3, 2, 1, 0]
        .iter()
        .filter(|&&i| hand[i] > 0)
        .map(|&i| {
            let n = hand[i] as u32;
            format!("{}{}", piece_char(HAND_KINDS[i]), if n > 1 { kanji_number(n) } else { String::new() })
        })
        .collect();
    if parts.is_empty() { "なし".to_owned() } else { parts.join("　") + "　" }
}

/// BOD board diagram (局面図).
pub fn bod(board: &Board, names: (&str, &str)) -> String {
    let mut s = format!("{}の持駒：{}\n", names.1, hand_text(&board.hands[1]));
    s.push_str("  ９ ８ ７ ６ ５ ４ ３ ２ １\n+---------------------------+\n");
    for rank in 1..=9 {
        s.push('|');
        for file in (1..=9).rev() {
            let p = board.squares[notation::square(file, rank)];
            if p == 0 {
                s.push_str(" ・");
            } else {
                s.push(if p >> 4 == 1 { 'v' } else { ' ' });
                s.push(piece_char(p & 15));
            }
        }
        s.push('|');
        s.push(notation::KANJI_DIGITS[rank]);
        s.push('\n');
    }
    s.push_str("+---------------------------+\n");
    s.push_str(&format!("{}の持駒：{}\n", names.0, hand_text(&board.hands[0])));
    if board.side == 1 {
        s.push_str(&format!("{}番\n", names.1));
    }
    s
}

/// Read a standalone BOD diagram (局面図) into a board.
pub fn read_bod(text: &str, limits: &Limits) -> Result<Board> {
    check_text(text, limits)?;
    let mut hs = HeaderState::default();
    for line in text.lines() {
        hs.take(line);
    }
    if hs.bod_rows.len() < 9 {
        return Err(ErrorKind::InvalidBoardDiagram(format!("{} rows", hs.bod_rows.len())).into());
    }
    hs.board()
}

fn push_comment(out: &mut String, comment: &str) {
    for c in comment.lines() {
        out.push('*');
        out.push_str(c);
        out.push('\n');
    }
}

fn kif_text(rec: &GameRecord, node: usize) -> String {
    let n = &rec.nodes[node];
    match (n.special, n.mv, n.parent) {
        (Some(s), _, _) => s.kif_name().to_owned(),
        (None, Some(mv), Some(parent)) => notation::kif_move(&rec.nodes[parent].board, mv, rec.prev_to(parent)),
        _ => String::new(),
    }
}

/// Cumulative time per node for the side that played it, in one pass (parents precede children).
pub(crate) fn cumulative_times(rec: &GameRecord) -> Vec<u64> {
    let mut cum = vec![0u64; rec.nodes.len()];
    for i in 1..rec.nodes.len() {
        let same_side_before = rec.nodes[i].parent.and_then(|p| rec.nodes[p].parent).map(|g| cum[g]).unwrap_or(0);
        cum[i] = same_side_before.saturating_add(rec.nodes[i].time_ms.unwrap_or(0));
    }
    cum
}

/// Visit every line in Kifu for Windows order: a line, then its branches deepest-first.
/// `emit(start, is_main)` writes the line starting at `start`; returns the nodes it covered.
fn for_each_line(rec: &GameRecord, mut emit: impl FnMut(usize, bool) -> Vec<usize>) {
    let Some(&first) = rec.nodes[0].children.first() else { return };
    let mut stack = vec![(first, true)];
    while let Some((start, is_main)) = stack.pop() {
        let chain = emit(start, is_main);
        // Alternatives of nodes deeper in this line come first.
        let mut pending = Vec::new();
        for &i in chain.iter().rev() {
            let Some(p) = rec.nodes[i].parent else { continue };
            let siblings = &rec.nodes[p].children;
            if siblings.first() == Some(&i) {
                pending.extend(siblings[1..].iter().copied());
            }
        }
        stack.extend(pending.into_iter().rev().map(|n| (n, false)));
    }
}

fn chain_from(rec: &GameRecord, start: usize) -> Vec<usize> {
    let mut chain = vec![start];
    let mut n = start;
    while let Some(&c) = rec.nodes[n].children.first() {
        chain.push(c);
        n = c;
    }
    chain
}

pub fn write_kif(rec: &GameRecord, opts: &WriteOptions) -> String {
    let mut out = format!("#KIF version=2.0 encoding={}\n", opts.encoding_label);
    write_headers(rec, &mut out);
    out.push_str("手数----指手---------消費時間--\n");
    push_comment(&mut out, &rec.nodes[0].comment);
    let cum = cumulative_times(rec);
    for_each_line(rec, |start, is_main| {
        if !is_main {
            out.push_str(&format!("\n変化：{}手\n", rec.nodes[start].ply));
        }
        let chain = chain_from(rec, start);
        for &i in &chain {
            let node = &rec.nodes[i];
            let text = kif_text(rec, i);
            let pad = 14usize.saturating_sub(display_width(&text));
            let has_variation = node
                .parent
                .is_some_and(|p| rec.nodes[p].children.len() > 1 && rec.nodes[p].children[0] == i);
            out.push_str(&format!(
                "{:>4} {}{} {}{}\n",
                node.ply,
                text,
                " ".repeat(pad),
                fmt_time(node.time_ms.unwrap_or(0), cum[i]),
                if has_variation { "+" } else { "" }
            ));
            push_comment(&mut out, &node.comment);
            if let Some(style) = opts.eval_style {
                for line in eval_comment::kif_lines(rec, i, style) {
                    out.push('*');
                    out.push_str(&line);
                    out.push('\n');
                }
            }
        }
        if is_main
            && let Some(s) = rec.result_sentence() {
                out.push_str(&s);
                out.push('\n');
            }
        chain
    });
    out
}

// ---------------------------------------------------------------- KI2

pub fn read_ki2(text: &str, limits: &Limits) -> Result<GameRecord> {
    check_text(text, limits)?;
    let mut hs = HeaderState::default();
    let mut lines = text.lines().enumerate().peekable();
    while let Some(&(_, line)) = lines.peek() {
        let t = line.trim();
        if t.starts_with(['▲', '△', '☗', '☖']) || variation_ply(t).is_some() {
            break;
        }
        lines.next();
        if t.is_empty() || t.starts_with('#') || t.starts_with('&') || t.starts_with("手数") {
            continue;
        }
        if let Some(c) = t.strip_prefix('*') {
            hs.root_comment.push(c.to_owned());
            continue;
        }
        hs.take(line);
    }
    let mut rec = hs.into_record()?;
    let mut cur = Cursor { path: vec![0] };
    for (no, line) in lines {
        let t = line.trim();
        let at = |e: Error| e.at(no + 1);
        if t.is_empty() || t.starts_with('#') || t.starts_with('&') {
            continue;
        }
        if let Some(c) = t.strip_prefix('*') {
            append_comment(&mut rec, cur.current(), c);
            continue;
        }
        if let Some(ply) = variation_ply(t) {
            cur.branch(ply).map_err(at)?;
            continue;
        }
        if let Some(result) = t.strip_prefix("まで") {
            let parent = cur.current();
            if rec.nodes[parent].special.is_some() {
                continue;
            }
            let sp = if result.contains("時間切れ") {
                Special::TimeUp
            } else if result.contains("反則") {
                Special::IllegalLoss
            } else if result.contains("入玉") {
                Special::EnteringKingWin
            } else if result.contains("勝ち") {
                Special::Resign
            } else if result.contains("千日手") {
                Special::Sennichite
            } else if result.contains("持将棋") {
                Special::Jishogi
            } else if result.contains("引き分け") {
                Special::Draw
            } else if result.contains("詰") {
                Special::Checkmate
            } else {
                Special::Interrupt
            };
            let node = rec.add_special(parent, sp);
            cur.path.push(node);
            continue;
        }
        for token in split_ki2_moves(t) {
            check_nodes(&rec, limits).map_err(at)?;
            let parent = cur.current();
            if rec.nodes[parent].special.is_some() {
                break;
            }
            let mv = notation::parse_jp_move(&rec.nodes[parent].board, &token, rec.prev_to(parent)).map_err(at)?;
            let node = rec.add_move(parent, mv).map_err(at)?;
            cur.path.push(node);
        }
    }
    eval_comment::extract_all(&mut rec);
    Ok(rec)
}

fn split_ki2_moves(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in line.chars() {
        if matches!(c, '▲' | '△' | '☗' | '☖') {
            out.push(String::new());
        } else if let Some(last) = out.last_mut() {
            last.push(c);
        }
    }
    out.into_iter().map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect()
}

pub fn write_ki2(rec: &GameRecord, opts: &WriteOptions) -> String {
    let mut out = String::new();
    write_headers(rec, &mut out);
    out.push('\n');
    push_comment(&mut out, &rec.nodes[0].comment);
    for_each_line(rec, |start, is_main| {
        if rec.nodes[start].special.is_some() && !is_main {
            return vec![start];
        }
        if !is_main {
            out.push_str(&format!("\n変化：{}手\n", rec.nodes[start].ply));
        }
        let chain = chain_from(rec, start);
        let mut col = 0;
        for &i in &chain {
            let node = &rec.nodes[i];
            if node.special.is_some() {
                continue;
            }
            let label = &node.label;
            out.push_str(label);
            col += 1;
            let evals: Vec<String> =
                opts.eval_style.map(|s| eval_comment::kif_lines(rec, i, s)).unwrap_or_default();
            if !node.comment.is_empty() || !evals.is_empty() || col == 6 || node.children.is_empty() {
                out.push('\n');
                col = 0;
            } else {
                out.push_str(&" ".repeat(10usize.saturating_sub(display_width(label))));
            }
            push_comment(&mut out, &node.comment);
            for line in evals {
                out.push('*');
                out.push_str(&line);
                out.push('\n');
            }
        }
        if col != 0 {
            out.push('\n');
        }
        if is_main
            && let Some(s) = rec.result_sentence() {
                out.push_str(&s);
                out.push('\n');
            }
        chain
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shogi_core::Move16;

    const SAMPLE: &str = "\
# ---- Kifu for Windows V7 V7.70 棋譜ファイル ----
開始日時：2024/01/02 10:00:00
棋戦：テスト棋戦
手合割：平手
先手：先手太郎
後手：後手花子
手数----指手---------消費時間--
*対局前コメント
   1 ７六歩(77)   ( 0:03/00:00:03)
*初手コメント
   2 ３四歩(33)   ( 0:05/00:00:05)
   3 ２二角成(88) ( 0:10/00:00:13)+
   4 同　銀(31)   ( 0:01/00:00:06)
   5 ４五角打     ( 1:02/00:01:15)
   6 投了         ( 0:02/00:00:08)
まで5手で先手の勝ち

変化：4手
   4 ８四歩(83)   ( 0:00/00:00:05)

変化：3手
   3 ６六歩(67)   ( 0:00/00:00:13)
   4 ８四歩(83)   ( 0:00/00:00:05)
";

    fn opts() -> WriteOptions {
        WriteOptions::default()
    }

    #[test]
    fn read_kif_with_variations() {
        let rec = read_kif(SAMPLE, &Limits::default()).unwrap();
        assert_eq!(rec.black_name(), Some("先手太郎"));
        assert_eq!(rec.header("棋戦"), Some("テスト棋戦"));
        assert_eq!(rec.nodes[0].comment, "対局前コメント");
        let main = rec.mainline();
        let labels: Vec<_> = main.iter().skip(1).map(|&n| rec.nodes[n].label.as_str()).collect();
        assert_eq!(labels, ["▲７六歩", "△３四歩", "▲２二角成", "△同　銀", "▲４五角", ""]);
        assert_eq!(rec.nodes[main[6]].special, Some(Special::Resign));
        assert_eq!(rec.nodes[main[1]].comment, "初手コメント");
        assert_eq!(rec.nodes[main[5]].time_ms, Some(62_000));
        // Kifu for Windows order: deeper branches of the current line come first.
        assert_eq!(rec.nodes[main[2]].children.len(), 2);
        let var3 = rec.nodes[main[2]].children[1];
        assert_eq!(rec.nodes[var3].label, "▲６六歩");
        assert_eq!(rec.nodes[var3].children.len(), 1);
        assert_eq!(rec.nodes[main[3]].children.len(), 2);
    }

    #[test]
    fn kif_roundtrip_preserves_tree() {
        let rec = read_kif(SAMPLE, &Limits::default()).unwrap();
        let text = write_kif(&rec, &opts());
        let again = read_kif(&text, &Limits::default()).unwrap();
        assert_eq!(write_kif(&again, &opts()), text);
        assert_eq!(again.nodes.len(), rec.nodes.len());
        assert!(text.contains("   4 同　銀(31)"));
        assert!(text.contains("( 0:10/00:00:13)+"));
        assert!(text.contains("まで5手で先手の勝ち"));
        assert!(text.contains("変化：3手"));
    }

    #[test]
    fn bod_roundtrip_and_handicap() {
        let sfen = "lnsgk2nl/1r4gs1/p1pppp1pp/1p4p2/9/2P4P1/PP1PPPP1P/2G4R1/LNS1KGSNL w Bb 20";
        let mut rec = GameRecord::from_sfen(sfen).unwrap();
        rec.add_move(0, Move16::from_usi("B*5e").unwrap()).unwrap();
        let text = write_kif(&rec, &opts());
        assert!(text.contains("後手番"));
        let again = read_kif(&text, &Limits::default()).unwrap();
        assert_eq!(again.initial().key(), rec.initial().key());
        assert_eq!(again.nodes[1].label, "△５五角");
        assert_eq!(read_bod(&bod(rec.initial(), ("先手", "後手")), &Limits::default()).unwrap().key(), rec.initial().key());

        let h = "手合割：角落ち\n下手：A\n上手：B\n手数----指手---------消費時間--\n   1 ３四歩(33)\n";
        let rec = read_kif(h, &Limits::default()).unwrap();
        assert!(rec.is_handicap());
        assert_eq!(rec.nodes[1].label, "△３四歩");
        assert!(write_kif(&rec, &opts()).contains("手合割：角落ち"));
    }

    #[test]
    fn ki2_roundtrip() {
        let rec = read_kif(SAMPLE, &Limits::default()).unwrap();
        let text = write_ki2(&rec, &opts());
        assert!(text.contains("▲７六歩"));
        assert!(text.contains("まで5手で先手の勝ち"));
        let again = read_ki2(&text, &Limits::default()).unwrap();
        assert_eq!(again.mainline().len(), rec.mainline().len());
        assert_eq!(write_ki2(&again, &opts()), text);
        assert_eq!(again.nodes.len(), rec.nodes.len());
    }

    #[test]
    fn ki2_external_sample() {
        let text = "\
開始日時：2023/05/01
先手：A
後手：B

▲２六歩    △８四歩    ▲２五歩    △８五歩    ▲７八金    △３二金
▲２四歩    △同　歩    ▲同　飛    △２三歩    ▲２六飛    △４一玉
*コメント
▲３八銀
まで13手で先手の勝ち
";
        let rec = read_ki2(text, &Limits::default()).unwrap();
        let main = rec.mainline();
        assert_eq!(main.len(), 15);
        assert_eq!(rec.nodes[main[8]].label, "△同　歩");
        assert_eq!(rec.nodes[main[12]].comment, "コメント");
        assert_eq!(rec.nodes[main[14]].special, Some(Special::Resign));
    }

    #[test]
    fn limits_and_deep_variations() {
        let limits = Limits { max_nodes: 4, ..Limits::default() };
        let e = read_kif(SAMPLE, &limits).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::TooLarge { what: "moves", .. }), "{e}");
        // 3000 nested variations: each branches from the previous variation's last move.
        let mut rec = GameRecord::from_sfen(shogi_core::START_SFEN).unwrap();
        let mut n = rec.add_usi(0, "7g7f").unwrap();
        let mut side_moves = ["3c3d", "4c4d"].iter().cycle();
        for i in 0..3000 {
            let a = rec.add_usi(n, if i % 2 == 0 { "1i1h" } else { "9i9h" }).unwrap_or(n);
            let _ = rec.add_usi(n, side_moves.next().unwrap());
            n = a;
            if rec.nodes[n].board.legal_moves().is_empty() {
                break;
            }
        }
        let text = write_kif(&rec, &opts());
        assert!(read_kif(&text, &Limits::default()).is_ok());
    }

    #[test]
    fn overflowing_time_is_ignored() {
        let t = "手数----指手--\n   1 ７六歩(77)   (99999999999999999999:01/00:00:01)\n";
        let rec = read_kif(t, &Limits::default()).unwrap();
        assert_eq!(rec.nodes[1].time_ms, None);
    }
}
