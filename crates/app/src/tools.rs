//! Conversion and mining tools shared by the GUI and the CLI: books, game records,
//! positions, game databases, and book mining. Files are read with size limits and written
//! atomically (temporary file + rename); existing files are replaced only when allowed.

use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sakiyomi_book::miner::{self, MinerConfig, MinerProgress};
use sakiyomi_book::{Book, BookFormat, Filter, MergePolicy, ReadOptions, Report, WriteOptions, peta};
use sakiyomi_kifu::db::{ConvertOptions, ConvertStats, DbFormat};
use sakiyomi_kifu::eval_comment::EvalStyle;
use sakiyomi_kifu::format::{self, Format, TextEncoding};
use sakiyomi_kifu::shogi_core::rules::Board;
use sakiyomi_kifu::{Limits, csa, kif, packed};
use sakiyomi_usi::EngineConfig;
use serde::{Deserialize, Serialize};

use crate::i18n::{book_error, kifu_error, tr, trf};
use crate::searcher::UsiSearcher;
use crate::spgio::SpgKind;
use sakiyomi_spg::BookSource;

/// Books can be large; game records are limited by `Limits::max_text_bytes`.
pub const MAX_BOOK_BYTES: u64 = 4 << 30;

pub fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let io = |e: std::io::Error| trf("読み書きできません: {err}", &[("err", &format!("{}: {e}", path.display()))]);
    let len = fs::metadata(path).map_err(io)?.len();
    if len > limit as u64 {
        return Err(trf("ファイルが大きすぎます: {path}", &[("path", &path.display())]));
    }
    let mut buf = Vec::with_capacity(len as usize);
    fs::File::open(path).map_err(io)?.take(limit as u64 + 1).read_to_end(&mut buf).map_err(io)?;
    if buf.len() > limit {
        return Err(trf("ファイルが大きすぎます: {path}", &[("path", &path.display())]));
    }
    Ok(buf)
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    path.with_file_name(format!(".{name}.{}-{nonce}.tmp", std::process::id()))
}

/// Write via a temporary file in the same directory, then rename over the target.
pub fn atomic_write(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    atomic_write_with(path, overwrite, |w| w.write_all(bytes).map_err(|e| e.to_string()))
}

/// Fail early, before any expensive work, when an output exists and may not be replaced.
/// (`atomic_write` checks again when it writes.)
pub fn check_output(path: &Path, overwrite: bool) -> Result<(), String> {
    if !overwrite && path.exists() {
        return Err(trf("既に存在します: {path}", &[("path", &path.display())]));
    }
    Ok(())
}

pub fn atomic_write_with(path: &Path, overwrite: bool, f: impl FnOnce(&mut BufWriter<fs::File>) -> Result<(), String>) -> Result<(), String> {
    check_output(path, overwrite)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !dir.is_dir() {
        return Err(trf("出力先がありません: {path}", &[("path", &dir.display())]));
    }
    let tmp = tmp_path(path);
    let io = |e: String| trf("読み書きできません: {err}", &[("err", &e)]);
    let result = (|| {
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| e.to_string())?;
        let mut w = BufWriter::new(file);
        f(&mut w)?;
        let file = w.into_inner().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// UTF-8 `.kif`/`.ki2` files get a BOM so other GUIs (which assume Shift_JIS) detect them.
pub fn encoding_for(path: &Path, fmt: Format, requested: TextEncoding) -> TextEncoding {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if requested == TextEncoding::Utf8 && matches!(fmt, Format::Kif | Format::Ki2) && (ext == "kif" || ext == "ki2") {
        TextEncoding::Utf8Bom
    } else {
        requested
    }
}

// ---------------------------------------------------------------- books

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BookConvertRequest {
    pub inputs: Vec<PathBuf>,
    /// Detected from contents/extension when absent.
    pub input_format: Option<String>,
    pub output: PathBuf,
    pub output_format: String,
    pub merge: String,
    pub min_count: Option<u64>,
    pub max_value_drop: Option<i32>,
    pub max_ply: Option<u32>,
    pub side: Option<u8>,
    pub top_moves: Option<usize>,
    /// Back up values through the book (peta shock) before writing.
    pub back_up: bool,
    pub sbk_evals_from_values: bool,
    /// Extra root positions (SFEN) for reading Gikou books.
    pub gikou_roots: Vec<String>,
    /// Every position of this book becomes a root when walking Gikou books.
    #[serde(default)]
    pub roots_from: Option<PathBuf>,
    /// The user's explicit choice not to write PROV when merging into SPG (§8.4). By default a
    /// merge into SPG records every source's contribution in PROV.
    pub no_prov: bool,
    pub overwrite: bool,
}

fn book_format(name: &str) -> Result<BookFormat, String> {
    BookFormat::from_name(name).ok_or_else(|| tr("未対応の形式です").to_owned())
}

pub fn read_book(path: &Path, fmt: Option<BookFormat>, opts: &ReadOptions) -> Result<(Book, Report), String> {
    let bytes = read_limited(path, MAX_BOOK_BYTES as usize)?;
    let fmt = fmt
        .or_else(|| path.extension().and_then(|e| e.to_str()).and_then(BookFormat::from_name))
        .or_else(|| BookFormat::detect(&bytes))
        .ok_or_else(|| tr("形式を判別できません").to_owned())?;
    // A trailing BOM is common in hand-edited .db files.
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    sakiyomi_book::read(fmt, bytes, opts).map_err(|e| book_error(&e))
}

pub fn convert_books(req: &BookConvertRequest) -> Result<Report, String> {
    if req.inputs.is_empty() {
        return Err(tr("入力ファイルがありません").to_owned());
    }
    check_output(&req.output, req.overwrite)?;
    let in_fmt = req.input_format.as_deref().filter(|f| SpgKind::from_name(f).is_none()).map(book_format).transpose()?;
    let out_spg = SpgKind::from_name(&req.output_format);
    let out_fmt = if out_spg.is_some() { BookFormat::YaneuraOuDb } else { book_format(&req.output_format)? };
    let policy = match req.merge.as_str() {
        "keepFirst" => MergePolicy::KeepFirst,
        "preferSecond" => MergePolicy::PreferSecond,
        _ => MergePolicy::Sum,
    };
    let mut opts = ReadOptions { roots: req.gikou_roots.clone(), ..Default::default() };
    if let Some(p) = &req.roots_from {
        let (b, _) = read_book(p, None, &ReadOptions::default())?;
        opts.roots.extend(b.positions.into_keys().map(|k| format!("{k} 1")));
    }
    let mut total = Report::default();
    let mut book: Option<Book> = None;
    let mut source: Option<BookFormat> = None;
    // Each input as read, for SRCS/PROV and rights when several are merged into SPG (§8.3, §8.4).
    let keep_sources = out_spg.is_some() && req.inputs.len() > 1;
    let mut inputs: Vec<(String, Option<BookFormat>, Book)> = Vec::new();
    for p in &req.inputs {
        let (b, r, fmt) = if SpgKind::of_path(p).is_some() {
            let (b, lost) = crate::spgio::read_book(p)?;
            total.lost_fields.extend(lost.0.into_iter().map(|(w, n)| format!("{w} ×{n}")));
            (b, Report::default(), None)
        } else {
            let fmt = in_fmt.or_else(|| p.extension().and_then(|e| e.to_str()).and_then(BookFormat::from_name));
            let (b, r) = read_book(p, fmt, &opts)?;
            source = source.or(fmt);
            (b, r, fmt)
        };
        if keep_sources {
            let name = p.file_name().map_or_else(|| p.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
            inputs.push((name, fmt, b.clone()));
        }
        total.illegal_moves += r.illegal_moves;
        total.invalid_positions += r.invalid_positions;
        total.duplicate_positions += r.duplicate_positions;
        total.unreachable += r.unreachable;
        match &mut book {
            None => book = Some(b),
            Some(acc) => sakiyomi_book::merge(acc, b, policy),
        }
    }
    let mut book = book.unwrap_or_default();
    sakiyomi_book::filter(
        &mut book,
        &Filter {
            min_count: req.min_count,
            max_value_drop: req.max_value_drop,
            max_ply: req.max_ply,
            side: req.side,
            top_moves: req.top_moves,
            value_range: None,
            drop_empty: true,
        },
    );
    if req.back_up {
        peta::back_up(&mut book, 64);
    }
    if let Some(kind) = out_spg {
        let mut after = Vec::new();
        let filters = [
            req.min_count.map(|n| format!("min count {n}")),
            req.max_value_drop.map(|n| format!("max value drop {n}")),
            req.max_ply.map(|n| format!("max ply {n}")),
            req.side.map(|s| format!("side {}", if s == 0 { "black" } else { "white" })),
            req.top_moves.map(|n| format!("top {n} moves")),
        ];
        let filters: Vec<String> = filters.into_iter().flatten().collect();
        if !filters.is_empty() {
            after.push(format!("filtered ({})", filters.join(", ")));
        }
        if req.back_up {
            after.push("values backed up through the book by minimax (PROV keeps each source's own values)".to_owned());
        }
        let sources: Vec<BookSource> = inputs.iter().map(|(name, format, book)| BookSource { name: name.clone(), format: *format, book }).collect();
        let merge = crate::spgio::BookMerge { sources: &sources, rule: crate::spgio::merge_rule(policy, &after), no_prov: req.no_prov };
        let data = crate::spgio::book_data(&book, source.unwrap_or(BookFormat::YaneuraOuDb), Some(merge));
        crate::spgio::write(&req.output, &data, kind, req.overwrite)?;
        total.positions = data.positions.len();
        total.moves = data.positions.iter().map(|p| p.edges.len()).sum();
        return Ok(total);
    }
    let (bytes, r) = sakiyomi_book::write(out_fmt, &book, &WriteOptions { sbk_evals_from_values: req.sbk_evals_from_values }).map_err(|e| book_error(&e))?;
    atomic_write(&req.output, &bytes, req.overwrite)?;
    total.positions = r.positions;
    total.moves = r.moves;
    total.lost_fields.extend(r.lost_fields);
    total.needs_roots = r.needs_roots;
    Ok(total)
}

// ---------------------------------------------------------------- game records

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KifuConvertRequest {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub format: String,
    /// "utf8" | "sjis"
    pub encoding: String,
    /// "shogigui" | "shogihome" | "floodgate" | "none"
    pub eval_style: String,
    pub overwrite: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KifuConvertSummary {
    pub converted: usize,
    pub failed: Vec<(String, String)>,
}

pub fn eval_style(name: &str) -> Option<EvalStyle> {
    EvalStyle::from_name(name)
}

pub fn text_encoding(name: &str) -> TextEncoding {
    match name {
        "sjis" | "shift_jis" | "cp932" => TextEncoding::ShiftJis,
        "utf8bom" | "utf-8-bom" => TextEncoding::Utf8Bom,
        _ => TextEncoding::Utf8,
    }
}

/// Records of a game-record file: SPG files may hold several.
fn read_records(input: &Path, limits: &Limits) -> Result<Vec<sakiyomi_kifu::record::GameRecord>, String> {
    if SpgKind::of_path(input).is_some() {
        return Ok(crate::spgio::read_records(input)?.records);
    }
    let bytes = read_limited(input, limits.max_text_bytes)?;
    let in_fmt = input.extension().and_then(|e| e.to_str()).and_then(Format::from_name);
    Ok(vec![format::read(&format::decode(&bytes), in_fmt, limits).map_err(|e| kifu_error(&e))?])
}

pub fn convert_kifu_files(req: &KifuConvertRequest, cancel: &AtomicBool, mut progress: impl FnMut(usize, usize)) -> Result<KifuConvertSummary, String> {
    let spg_out = SpgKind::from_name(&req.format).filter(|k| *k != SpgKind::Manifest);
    let fmt = if spg_out.is_some() { Format::Kif } else { Format::from_name(&req.format).ok_or_else(|| tr("未対応の形式です").to_owned())? };
    let enc = text_encoding(&req.encoding);
    let style = eval_style(&req.eval_style);
    if !req.out_dir.is_dir() {
        return Err(trf("出力先がありません: {path}", &[("path", &req.out_dir.display())]));
    }
    let limits = Limits::default();
    let mut summary = KifuConvertSummary::default();
    let ext = match (fmt, enc) {
        _ if spg_out == Some(SpgKind::Binary) => "spg",
        _ if spg_out == Some(SpgKind::Text) => "spgt",
        (Format::Kif, TextEncoding::ShiftJis) => "kif",
        (Format::Kif, _) => "kifu",
        (Format::Ki2, TextEncoding::ShiftJis) => "ki2",
        (Format::Ki2, _) => "ki2u",
        (Format::Csa, _) => "csa",
        (Format::Jkf, _) => "jkf",
        (Format::Usi, _) => "sfen",
    };
    for (i, input) in req.inputs.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(tr("中止しました").to_owned());
        }
        progress(i, req.inputs.len());
        let result = (|| {
            let recs = read_records(input, &limits)?;
            // Only the file name of the input is used: outputs never escape `out_dir`.
            let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("record{i}"));
            if let Some(kind) = spg_out {
                let out = req.out_dir.join(format!("{stem}.{ext}"));
                return atomic_write(&out, &crate::spgio::records_to_bytes(&recs, kind == SpgKind::Text)?, req.overwrite);
            }
            for (k, rec) in recs.iter().enumerate() {
                let text = format::write(rec, fmt, enc, style).map_err(|e| kifu_error(&e))?;
                let name = if recs.len() == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{}.{ext}", k + 1) };
                let out = req.out_dir.join(name);
                atomic_write(&out, &format::encode(&text, encoding_for(&out, fmt, enc)), req.overwrite)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => summary.converted += 1,
            Err(e) => summary.failed.push((input.display().to_string(), e)),
        }
    }
    progress(req.inputs.len(), req.inputs.len());
    Ok(summary)
}

// ---------------------------------------------------------------- positions

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PositionFormat {
    Sfen,
    Usi,
    Bod,
    Csa,
    /// HuffmanCodedPos as 64 hex digits.
    Hcp,
    /// YaneuraOu PackedSfen as 64 hex digits.
    Psfen,
}

fn unhex32(s: &str) -> Result<[u8; 32], String> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if t.len() != 64 || !t.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(tr("16進数32バイトではありません").to_owned());
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn detect_position(t: &str) -> Option<PositionFormat> {
    let s = t.trim();
    if s.starts_with("position") || s.starts_with("startpos") || s.contains(" moves ") {
        Some(PositionFormat::Usi)
    } else if s.lines().any(|l| l.trim_start().starts_with('|')) {
        Some(PositionFormat::Bod)
    } else if s.starts_with("PI") || s.starts_with("P1") || s.starts_with("P+") || s.starts_with("V2") {
        Some(PositionFormat::Csa)
    } else if s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()) {
        Some(PositionFormat::Hcp)
    } else if s.split_whitespace().next().is_some_and(|f| f.matches('/').count() == 8) || s.starts_with("sfen ") {
        Some(PositionFormat::Sfen)
    } else {
        None
    }
}

pub fn convert_position(text: &str, from: Option<PositionFormat>, to: PositionFormat) -> Result<String, String> {
    let limits = Limits::default();
    let from = from.or_else(|| detect_position(text)).ok_or_else(|| tr("局面形式を判別できません").to_owned())?;
    let board: Board = match from {
        PositionFormat::Sfen => sakiyomi_kifu::record::parse_sfen(text.trim().trim_start_matches("sfen ")).map_err(|e| kifu_error(&e))?,
        PositionFormat::Usi => {
            let rec = format::read_usi(text, &limits).map_err(|e| kifu_error(&e))?;
            rec.nodes[rec.line_end(0)].board
        }
        PositionFormat::Bod => kif::read_bod(text, &limits).map_err(|e| kifu_error(&e))?,
        PositionFormat::Csa => csa::read_csa_position(text, &limits).map_err(|e| kifu_error(&e))?,
        PositionFormat::Hcp => packed::from_hcp(&unhex32(text)?).map_err(|e| kifu_error(&e))?,
        PositionFormat::Psfen => packed::from_psfen(&unhex32(text)?).map_err(|e| kifu_error(&e))?,
    };
    let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let bin_err = |e: sakiyomi_kifu::Error| trf("この局面はバイナリ形式にできません: {err}", &[("err", &kifu_error(&e))]);
    Ok(match to {
        PositionFormat::Sfen => board.to_sfen(),
        PositionFormat::Usi => format!("position sfen {}", board.to_sfen()),
        PositionFormat::Bod => kif::bod(&board, ("先手", "後手")),
        PositionFormat::Csa => csa::csa_position(&board),
        PositionFormat::Hcp => hex(packed::to_hcp(&board).map_err(bin_err)?),
        PositionFormat::Psfen => hex(packed::to_psfen(&board).map_err(bin_err)?),
    })
}

// ---------------------------------------------------------------- databases

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DbConvertRequest {
    pub input: PathBuf,
    pub input_format: Option<String>,
    pub output: PathBuf,
    pub output_format: String,
    pub skip_without_eval: bool,
    pub min_ply: u16,
    pub overwrite: bool,
}

pub fn convert_db(req: &DbConvertRequest, cancel: &AtomicBool, mut progress: impl FnMut(&ConvertStats)) -> Result<ConvertStats, String> {
    check_output(&req.output, req.overwrite)?;
    let spg_in = req.input_format.as_deref().and_then(SpgKind::from_name).or_else(|| SpgKind::of_path(&req.input));
    let spg_out = SpgKind::from_name(&req.output_format);
    if spg_in.is_some() || spg_out.is_some() {
        return convert_db_spg(req, spg_in, spg_out, cancel, progress);
    }
    let from = req
        .input_format
        .as_deref()
        .and_then(DbFormat::from_name)
        .or_else(|| req.input.extension().and_then(|e| e.to_str()).and_then(DbFormat::from_name))
        .ok_or_else(|| tr("形式を判別できません").to_owned())?;
    let to = DbFormat::from_name(&req.output_format).ok_or_else(|| tr("未対応の形式です").to_owned())?;
    let io = |e: std::io::Error| trf("読み書きできません: {err}", &[("err", &e)]);
    let input = BufReader::with_capacity(1 << 20, fs::File::open(&req.input).map_err(io)?);
    let opts = ConvertOptions { skip_without_eval: req.skip_without_eval, min_ply: req.min_ply };
    let mut stats = ConvertStats::default();
    let mut cancelled = false;
    atomic_write_with(&req.output, req.overwrite, |w| {
        stats = sakiyomi_kifu::db::convert(from, input, to, w, &opts, &Limits::default(), |s| {
            if (s.games + s.positions_written) % 1024 == 0 {
                progress(s);
            }
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                return false;
            }
            true
        })
        .map_err(|e| kifu_error(&e))?;
        if cancelled { Err(tr("中止しました").to_owned()) } else { Ok(()) }
    })?;
    progress(&stats);
    Ok(stats)
}

/// Game databases to or from SPG. SPG v1.0 files are written in one go (§11.7), so the
/// games are collected in memory.
fn convert_db_spg(req: &DbConvertRequest, spg_in: Option<SpgKind>, spg_out: Option<SpgKind>, cancel: &AtomicBool, mut progress: impl FnMut(&ConvertStats)) -> Result<ConvertStats, String> {
    use sakiyomi_kifu::db;
    let mut stats = ConvertStats::default();
    let games: Vec<sakiyomi_kifu::record::GameRecord> = if spg_in.is_some() {
        let r = crate::spgio::read_records(&req.input)?;
        stats.skipped = r.errors.len() as u64;
        r.records
    } else {
        let from = req.input_format.as_deref().and_then(DbFormat::from_name).or_else(|| req.input.extension().and_then(|e| e.to_str()).and_then(DbFormat::from_name)).ok_or_else(|| tr("形式を判別できません").to_owned())?;
        if !from.is_game_format() {
            return Err(tr("局面単位の形式から棋譜は作れません").to_owned());
        }
        let io = |e: std::io::Error| trf("読み書きできません: {err}", &[("err", &e)]);
        let input = BufReader::with_capacity(1 << 20, fs::File::open(&req.input).map_err(io)?);
        let mut out = Vec::new();
        for g in db::GameReader::new(from, input, Limits::default()) {
            if cancel.load(Ordering::Relaxed) {
                return Err(tr("中止しました").to_owned());
            }
            match g.and_then(|g| db::game_to_record(&g)) {
                Ok(r) => out.push(r),
                Err(_) => stats.skipped += 1,
            }
            stats.games += 1;
            if stats.games % 1024 == 0 {
                progress(&stats);
            }
        }
        out
    };
    if let Some(kind) = spg_out {
        let data = sakiyomi_spg::records_to_spg(&games, &sakiyomi_spg::record::Origin { format: req.input_format.clone().unwrap_or_default(), raw: None });
        crate::spgio::write(&req.output, &data, kind, req.overwrite)?;
        stats.games = games.len() as u64;
        progress(&stats);
        return Ok(stats);
    }
    let to = DbFormat::from_name(&req.output_format).ok_or_else(|| tr("未対応の形式です").to_owned())?;
    let opts = ConvertOptions { skip_without_eval: req.skip_without_eval, min_ply: req.min_ply };
    atomic_write_with(&req.output, req.overwrite, |w| {
        for rec in &games {
            if cancel.load(Ordering::Relaxed) {
                return Err(tr("中止しました").to_owned());
            }
            let g = db::record_to_game(rec);
            stats.games += 1;
            if to.is_game_format() {
                db::write_game(to, w, &g).map_err(|e| kifu_error(&e))?;
                continue;
            }
            for (p, m) in db::game_positions(&g).map_err(|e| kifu_error(&e))?.iter().zip(&g.moves) {
                db::emit_position(to, w, p, m.eval.is_some(), &opts, &mut stats).map_err(|e| kifu_error(&e))?;
            }
        }
        Ok(())
    })?;
    progress(&stats);
    Ok(stats)
}

// ---------------------------------------------------------------- mining

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MineRequest {
    /// Existing raw book to continue (YaneuraOu .db).
    pub book_in: Option<PathBuf>,
    /// Raw book (leaf values) written after every round, for resuming.
    pub raw_out: PathBuf,
    /// Backed-up, playable book written at the end.
    pub book_out: Option<PathBuf>,
    pub workers: usize,
    /// SFEN roots; empty = the initial position.
    pub roots: Vec<String>,
    pub nodes: u64,
    pub multipv: usize,
    pub multipv_delta: i32,
    pub eval_diff: i32,
    pub max_ply: u32,
    pub leaves_per_round: usize,
    pub rounds: usize,
    pub black: bool,
    pub white: bool,
}

impl Default for MineRequest {
    fn default() -> Self {
        let c = MinerConfig::default();
        MineRequest {
            book_in: None,
            raw_out: PathBuf::new(),
            book_out: None,
            workers: 1,
            roots: Vec::new(),
            nodes: c.nodes,
            multipv: c.multipv,
            multipv_delta: c.multipv_delta,
            eval_diff: c.eval_diff,
            max_ply: c.max_ply,
            leaves_per_round: c.leaves_per_round,
            rounds: c.rounds,
            black: true,
            white: true,
        }
    }
}

pub fn mine(req: &MineRequest, engine: &EngineConfig, cancel: &Arc<AtomicBool>, mut progress: impl FnMut(&MinerProgress)) -> Result<MinerProgress, String> {
    if req.workers == 0 {
        return Err(tr("ワーカー数は1以上にしてください").to_owned());
    }
    // A raw book that does not exist yet means a fresh start; an existing one is continued.
    let mut book = match &req.book_in {
        Some(p) if p.exists() => read_book(p, Some(BookFormat::YaneuraOuDb), &ReadOptions::default())?.0,
        _ => Book::default(),
    };
    let mut roots = Vec::new();
    for r in &req.roots {
        roots.push(sakiyomi_kifu::record::parse_sfen(r).map_err(|e| kifu_error(&e))?);
    }
    if roots.is_empty() {
        roots.push(sakiyomi_kifu::record::parse_sfen(sakiyomi_kifu::shogi_core::START_SFEN).map_err(|e| kifu_error(&e))?);
    }
    let mut searchers: Vec<Box<dyn miner::Searcher>> = Vec::new();
    for _ in 0..req.workers {
        searchers.push(Box::new(UsiSearcher::start(engine, cancel.clone())?));
    }
    let cfg = MinerConfig {
        nodes: req.nodes,
        multipv: req.multipv,
        multipv_delta: req.multipv_delta,
        eval_diff: req.eval_diff,
        max_ply: req.max_ply,
        max_step: 1000,
        leaves_per_round: req.leaves_per_round,
        rounds: req.rounds,
        sides: [req.black, req.white],
    };
    let raw_out = req.raw_out.clone();
    let mut save_error = None;
    let p = miner::mine(&mut book, &roots, searchers, &cfg, cancel, |p| progress(p), |b| {
        if let Err(e) = atomic_write(&raw_out, &sakiyomi_book::db::write(b), true) {
            save_error = Some(e);
        }
    });
    if let Some(e) = save_error {
        return Err(e);
    }
    atomic_write(&raw_out, &sakiyomi_book::db::write(&book), true)?;
    if let Some(out) = &req.book_out {
        let mut playable = book.clone();
        peta::back_up(&mut playable, 64);
        atomic_write(out, &sakiyomi_book::db::write(&playable), true)?;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_conversions() {
        let start = sakiyomi_kifu::shogi_core::START_SFEN;
        let hcp = convert_position(start, None, PositionFormat::Hcp).unwrap();
        assert_eq!(hcp, "58a449210cd757217e8e4d212caf427814c2ab109e4d212cd75f213e8e49210c");
        assert_eq!(convert_position(&hcp, Some(PositionFormat::Hcp), PositionFormat::Sfen).unwrap(), start);
        let bod = convert_position(start, None, PositionFormat::Bod).unwrap();
        assert_eq!(convert_position(&bod, None, PositionFormat::Sfen).unwrap(), start);
        let csa = convert_position("position startpos moves 7g7f", None, PositionFormat::Csa).unwrap();
        assert!(csa.contains("P7+FU+FU * +FU"));
        assert!(convert_position("4k4/9/9/9/9/9/9/9/4K4 b - 1", None, PositionFormat::Hcp).is_ok());
        assert!(convert_position("zz", Some(PositionFormat::Hcp), PositionFormat::Sfen).is_err());
    }

    #[test]
    fn atomic_write_policy() {
        let dir = std::env::temp_dir().join(format!("sakiyomi-aw-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        atomic_write(&f, b"one", false).unwrap();
        assert!(atomic_write(&f, b"two", false).is_err());
        assert_eq!(fs::read(&f).unwrap(), b"one");
        atomic_write(&f, b"two", true).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"two");
        assert!(fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
        assert!(atomic_write(&dir.join("missing/x.txt"), b"x", true).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
