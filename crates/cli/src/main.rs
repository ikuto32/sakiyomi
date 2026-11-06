//! `sakiyomi-tool`: command-line conversions and book mining. Every command is a thin
//! wrapper over `sakiyomi_app::tools`, the same functions the GUI uses.
//!
//! ```text
//! sakiyomi-tool book convert a.sbk b.db -o merged.db --min-count 2
//! sakiyomi-tool book stats book.db
//! sakiyomi-tool book mine --engine engine.exe --option Threads=1 --nodes 300000 -o raw.db --book book.db
//! sakiyomi-tool kifu convert games/*.kif --out-dir out --to csa --eval floodgate
//! sakiyomi-tool position convert "position startpos moves 7g7f" --to hcp
//! sakiyomi-tool db convert games.hcpe3 -o train.hcpe --to hcpe --skip-without-eval
//! ```
//!
//! Existing output files are never replaced unless `--force` is given.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};
use sakiyomi_app::tools::{self, BookConvertRequest, DbConvertRequest, KifuConvertRequest, MineRequest, PositionFormat};
use sakiyomi_usi::EngineConfig;

#[derive(Parser)]
#[command(name = "sakiyomi-tool", version, about = "Shogi book, record, position and database conversion; book mining")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// SPG (Shogi Position Graph) v1.0, the standard format: verify, convert, inspect
    #[command(subcommand)]
    Spg(SpgCmd),
    /// Opening books (SPG / YaneuraOu .db / ShogiGUI .sbk / Gikou book.bin)
    #[command(subcommand)]
    Book(BookCmd),
    /// Game records (SPG / KIF / KI2 / CSA / JKF / USI)
    #[command(subcommand)]
    Kifu(KifuCmd),
    /// Single positions (SFEN / USI / BOD / CSA / hcp / psfen)
    #[command(subcommand)]
    Position(PositionCmd),
    /// Game and training databases (SPG / hcpe / psv / hcpe3 / SFEN lines / CSA multi-game)
    #[command(subcommand)]
    Db(DbCmd),
}

#[derive(Subcommand)]
enum BookCmd {
    /// Convert, merge and filter books
    Convert {
        /// Input books (merged in order)
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
        /// Output format: spg, spgt, spgm, db, sbk, gikou (default: from the output extension; .bin = gikou)
        #[arg(long)]
        to: Option<String>,
        /// Input format (default: detected from content)
        #[arg(long)]
        from: Option<String>,
        /// How duplicate moves merge: sum | keepFirst | preferSecond
        #[arg(long, default_value = "sum")]
        merge: String,
        /// Drop moves played fewer times than this
        #[arg(long)]
        min_count: Option<u64>,
        /// Drop moves whose value is worse than the best by more than this
        #[arg(long)]
        max_value_drop: Option<i32>,
        /// Drop positions beyond this ply
        #[arg(long)]
        max_ply: Option<u32>,
        /// Keep only positions with this side to move: black | white
        #[arg(long)]
        side: Option<String>,
        /// Keep at most this many moves per position
        #[arg(long)]
        top: Option<usize>,
        /// Back up values through the book (minimax, "peta shock") before writing
        #[arg(long)]
        back_up: bool,
        /// .sbk output: do not store move values as engine evaluations of the positions
        #[arg(long)]
        no_sbk_evals: bool,
        /// SPG output merging several inputs: do not write PROV (each source's contribution).
        /// PROV is written unless this is given (SPG §8.4)
        #[arg(long)]
        no_prov: bool,
        /// Extra root positions (SFEN) when reading Gikou books (repeatable)
        #[arg(long = "root")]
        roots: Vec<String>,
        /// Use every position of this book as a root when reading Gikou books
        #[arg(long)]
        roots_from: Option<PathBuf>,
        /// Replace an existing output file
        #[arg(long)]
        force: bool,
    },
    /// Print statistics of a book
    Stats { input: PathBuf },
    /// Mine a book with a USI engine (BookMiner-style). The raw book is saved after every
    /// round, so an interrupted run continues where it stopped.
    Mine {
        /// Engine executable
        #[arg(long)]
        engine: PathBuf,
        /// Engine option NAME=VALUE (repeatable), e.g. --option Threads=1
        #[arg(long = "option")]
        options: Vec<String>,
        /// Raw book with leaf values; continued if it exists
        #[arg(short, long)]
        output: PathBuf,
        /// Playable book with backed-up values, written at the end
        #[arg(long)]
        book: Option<PathBuf>,
        /// Parallel engine processes
        #[arg(long, default_value_t = 1)]
        workers: usize,
        /// Node budget per search
        #[arg(long, default_value_t = 1_000_000)]
        nodes: u64,
        /// Rounds to run (0 = until no leaves remain)
        #[arg(long, default_value_t = 0)]
        rounds: usize,
        /// Leaves searched per round
        #[arg(long, default_value_t = 64)]
        leaves: usize,
        /// Opponent moves within this many cp of the best are expanded
        #[arg(long, default_value_t = 30)]
        eval_diff: i32,
        /// Initial MultiPV (widened while candidates are close)
        #[arg(long, default_value_t = 4)]
        multipv: usize,
        #[arg(long, default_value_t = 200)]
        max_ply: u32,
        /// Root positions (SFEN or USI position; default: the initial position)
        #[arg(long = "root")]
        roots: Vec<String>,
        /// Only expand positions where Black (Sente) is the book side
        #[arg(long, conflicts_with = "white_only")]
        black_only: bool,
        /// Only expand positions where White (Gote) is the book side
        #[arg(long)]
        white_only: bool,
    },
}

#[derive(Subcommand)]
enum KifuCmd {
    /// Convert record files into a directory
    Convert {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long)]
        out_dir: PathBuf,
        /// spg | spgt | kif | ki2 | csa | jkf | usi
        #[arg(long)]
        to: String,
        /// utf8 | utf8bom | sjis
        #[arg(long, default_value = "utf8")]
        encoding: String,
        /// Evaluation comments: shogigui | shogihome | floodgate | none
        #[arg(long, default_value = "shogigui")]
        eval: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum PositionCmd {
    /// Convert one position given as an argument or on standard input
    Convert {
        text: Option<String>,
        /// sfen | usi | bod | csa | hcp | psfen
        #[arg(long)]
        to: String,
        /// Input format (default: detected)
        #[arg(long)]
        from: Option<String>,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Convert a game/position database
    Convert {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// spg | spgt | spgm | hcpe | psv | hcpe3 | sfen | csa
        #[arg(long)]
        to: String,
        #[arg(long)]
        from: Option<String>,
        /// Skip positions without an evaluation
        #[arg(long)]
        skip_without_eval: bool,
        /// Skip positions before this ply
        #[arg(long, default_value_t = 0)]
        min_ply: u16,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum SpgCmd {
    /// Check a .spg / .spgt / .spgm against every requirement of the specification
    Verify {
        input: PathBuf,
        /// Skip the move-legality checks (much faster on large books)
        #[arg(long)]
        no_legality: bool,
    },
    /// Convert between .spg, .spgt and .spgm (chosen by the output extension)
    Convert {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Edge form of .spg/.spgm output: auto | full | compact
        #[arg(long, default_value = "auto")]
        edges: String,
        /// Add BLK3 tamper-detection hashes
        #[arg(long)]
        blk3: bool,
        /// .spgm: split into 256 position shards
        #[arg(long)]
        split: bool,
        /// .spgm: sign the manifest with this Ed25519 key (32 raw bytes or 64 hex digits)
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// .spgm: delete the previous build's files after the manifests are replaced
        #[arg(long)]
        remove_old: bool,
        #[arg(long)]
        force: bool,
    },
    /// Summary of a file
    Info { input: PathBuf },
    /// Look a position up the way an engine does (no full decoding)
    Probe {
        input: PathBuf,
        /// SFEN (with or without the move number); default: the initial position
        sfen: Option<String>,
    },
    /// Write the original files kept in RAWS into a directory
    Raws {
        input: PathBuf,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

const STDIN_LIMIT: u64 = 1 << 20;

fn spg_read_opts(legality: bool) -> spg::ReadOptions {
    spg::ReadOptions { legality, ..Default::default() }
}

fn sakiyomi_spg_read(path: &std::path::Path, legality: bool) -> Result<spg::Spg, String> {
    let limit = tools::MAX_BOOK_BYTES;
    if path.extension().is_some_and(|e| e == "spgm") {
        return spg::manifest::read_set(path, &spg_read_opts(legality), limit).map(|(s, _)| s).map_err(|e| e.to_string());
    }
    let bytes = tools::read_limited(path, limit as usize)?;
    match spg::sniff(&bytes) {
        Some("spg") => spg::read(&bytes, &spg_read_opts(legality)).map(|(s, _)| s).map_err(|e| e.to_string()),
        Some("spgt") => spg::text::parse(std::str::from_utf8(&bytes).map_err(|_| "not UTF-8")?).map_err(|e| e.to_string()),
        _ => Err("not an SPG file".into()),
    }
}

fn signing_key(path: &std::path::Path) -> Result<[u8; 32], String> {
    let b = tools::read_limited(path, 256)?;
    if b.len() == 32 {
        return b.try_into().map_err(|_| "bad key".to_owned());
    }
    let t: String = String::from_utf8_lossy(&b).split_whitespace().collect();
    if t.len() != 64 || !t.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("the key file must hold 32 bytes or 64 hex digits".into());
    }
    let mut k = [0u8; 32];
    for (i, o) in k.iter_mut().enumerate() {
        *o = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(k)
}

fn run_spg(cmd: SpgCmd) -> Result<(), String> {
    match cmd {
        SpgCmd::Verify { input, no_legality } => {
            let s = sakiyomi_spg_read(&input, !no_legality)?;
            let edges: usize = s.positions.iter().map(|p| p.edges.len()).sum();
            println!("OK: {} positions, {edges} edges, {} records{}", s.positions.len(), s.records.len(), if no_legality { " (legality not checked)" } else { "" });
        }
        SpgCmd::Convert { input, output, edges, blk3, split, sign_key, remove_old, force } => {
            let s = sakiyomi_spg_read(&input, true)?;
            let edges = match edges.as_str() {
                "auto" => spg::EdgeForm::Auto,
                "full" => spg::EdgeForm::Full,
                "compact" => spg::EdgeForm::Compact,
                o => return Err(format!("unknown edge form: {o}")),
            };
            // Rewriting keeps what a newer minor version added (§7.6, §8.2).
            let checks = spg::validate::Checks { allow_unknown: true, legality: true };
            let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            match ext.as_str() {
                "spgt" => tools::atomic_write(&output, spg::text::write(&s).map_err(|e| e.to_string())?.as_bytes(), force)?,
                "spgm" => {
                    tools::check_output(&output, force)?;
                    let dir = output.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    let name = output.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".spgm")).ok_or("the manifest is named <name>.spgm")?;
                    let key = sign_key.as_deref().map(signing_key).transpose()?;
                    let rep = spg::manifest::write_set(dir, name, &s, &spg::manifest::SetOptions { split, edges, blk3, signing_key: key, checks, remove_old, ..Default::default() }).map_err(|e| e.to_string())?;
                    println!("build {:016x}: {} files", rep.build_id, rep.files.len());
                    if !rep.old_files.is_empty() && !rep.removed_old {
                        println!("previous build kept ({} files); remove them once no reader uses them, or pass --remove-old", rep.old_files.len());
                    }
                }
                _ => {
                    let (bytes, rep) = spg::write(&s, &spg::WriteOptions { edges, blk3, checks, ..Default::default() }).map_err(|e| e.to_string())?;
                    tools::atomic_write(&output, &bytes, force)?;
                    if rep.dropped_weights + rep.dropped_stats > 0 {
                        eprintln!("compact edges: dropped {} weights and {} statistics (recorded in src.partial)", rep.dropped_weights, rep.dropped_stats);
                    }
                }
            }
            println!("{} positions, {} records", s.positions.len(), s.records.len());
        }
        SpgCmd::Info { input } => {
            if input.extension().is_some_and(|e| e == "spgm") {
                let (s, info) = spg::manifest::read_set(&input, &spg_read_opts(false), tools::MAX_BOOK_BYTES).map_err(|e| e.to_string())?;
                let m = info.manifest.ok_or("no manifest")?;
                println!("manifest v1.{} build {:016x}, s = {}, {} files, {} positions, {} records", m.minor, m.build_id, m.s, m.entries.len(), m.total_positions, s.records.len());
                println!("signed: {}", match info.signed_by { Some(k) => k.iter().map(|b| format!("{b:02x}")).collect::<String>(), None => "no".into() });
                if info.used_backup {
                    println!("note: .spgm was damaged or stale; .spgm.bak was used");
                }
                return Ok(());
            }
            let bytes = tools::read_limited(&input, tools::MAX_BOOK_BYTES as usize)?;
            if spg::sniff(&bytes) == Some("spgt") {
                let s = sakiyomi_spg_read(&input, false)?;
                println!("SPG text: {} positions, {} records, {} engines", s.positions.len(), s.records.len(), s.engines.len());
                return Ok(());
            }
            let c = spg::container::Container::open(&bytes).map_err(|e| e.to_string())?;
            let h = &c.header;
            println!("SPG v{}.{} {:?} build {:016x}{}", h.major, h.minor, h.kind, h.build_id, if c.used_tail_copy { " (header repaired from the tail copy)" } else { "" });
            for e in &c.entries {
                let (count, rec, _) = c.prelude(c.index(e.tag).unwrap_or(0));
                println!("  {} {:>12} bytes  {:>10} × {:>2}{}{}", String::from_utf8_lossy(&e.tag), e.len, count, rec, if e.flags & 1 != 0 { "  required" } else { "" }, if e.flags & 2 != 0 { "  derived" } else { "" });
            }
        }
        SpgCmd::Probe { input, sfen } => {
            let bytes = tools::read_limited(&input, tools::MAX_BOOK_BYTES as usize)?;
            let book = spg::lookup::Book::open(&bytes).map_err(|e| e.to_string())?;
            let pos = match sfen {
                Some(s) => spg::Position::parse_sfen(s.trim().trim_start_matches("sfen ")).map_err(|e| e.to_string())?.0,
                None => spg::Position::startpos(),
            };
            match book.find(&pos).map_err(|e| e.to_string())? {
                None => println!("not in the book"),
                Some(k) => {
                    for e in book.edges(k).map_err(|e| e.to_string())? {
                        let v = e.eval.map_or("-".into(), |v| spg::eval::to_text(v.value));
                        println!("{:<6} eval {:>8} weight {:>8} {}", e.mv.to_usi(), v, e.weight.map_or("-".into(), |w| w.to_string()), if e.disabled { "(disabled)" } else { "" });
                    }
                }
            }
        }
        SpgCmd::Raws { input, out_dir, force } => {
            let s = sakiyomi_spg_read(&input, false)?;
            std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
            for (i, r) in s.raws.iter().enumerate() {
                // Only the file name is used, so a stored name cannot escape out_dir.
                let name = r.filename.as_deref().and_then(|f| std::path::Path::new(f).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("raw{i}.{}", r.format));
                let path = out_dir.join(format!("{i:04}-{name}"));
                tools::atomic_write(&path, &r.original().map_err(|e| e.to_string())?, force)?;
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}

fn position_format(s: &str) -> Result<PositionFormat, String> {
    serde_json::from_value(serde_json::Value::String(s.to_ascii_lowercase())).map_err(|_| format!("unknown position format: {s}"))
}

fn side(s: Option<&str>) -> Result<Option<u8>, String> {
    match s.map(str::to_ascii_lowercase).as_deref() {
        None => Ok(None),
        Some("black" | "b" | "sente") => Ok(Some(0)),
        Some("white" | "w" | "gote") => Ok(Some(1)),
        Some(o) => Err(format!("unknown side: {o}")),
    }
}

fn book_format_from_path(p: &std::path::Path) -> Option<String> {
    match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "bin" => Some("gikou".into()),
        e => Some(e.to_owned()),
    }
}

fn engine_config(path: PathBuf, options: Vec<String>) -> Result<EngineConfig, String> {
    let id = sakiyomi_usi::registry::probe(&path)?;
    let mut values = BTreeMap::new();
    for o in options {
        let (k, v) = o.split_once('=').ok_or_else(|| format!("--option expects NAME=VALUE: {o}"))?;
        if !id.options.iter().any(|d| d.name == k) {
            return Err(format!("the engine has no option named {k}"));
        }
        values.insert(k.to_owned(), v.to_owned());
    }
    Ok(EngineConfig {
        id: "cli".into(),
        name: id.name.clone(),
        path: path.to_string_lossy().into_owned(),
        author: id.author,
        usi_name: id.name,
        options: id.options,
        values,
    })
}

fn run(cli: Cli) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    match cli.cmd {
        Cmd::Spg(c) => run_spg(c)?,
        Cmd::Book(BookCmd::Convert { inputs, output, to, from, merge, min_count, max_value_drop, max_ply, side: s, top, back_up, no_sbk_evals, no_prov, roots, roots_from, force }) => {
            let to = to.or_else(|| book_format_from_path(&output)).ok_or("--to is required")?;
            let req = BookConvertRequest {
                inputs,
                input_format: from,
                output,
                output_format: to,
                merge,
                min_count,
                max_value_drop,
                max_ply,
                side: side(s.as_deref())?,
                top_moves: top,
                back_up,
                sbk_evals_from_values: !no_sbk_evals,
                gikou_roots: roots,
                roots_from,
                no_prov,
                overwrite: force,
            };
            println!("{}", tools::convert_books(&req)?);
        }
        Cmd::Book(BookCmd::Stats { input }) => {
            let (book, report) = tools::read_book(&input, None, &Default::default())?;
            println!("{report}");
            println!("{}", serde_json::to_string_pretty(&sakiyomi_book::stats(&book)).map_err(|e| e.to_string())?);
        }
        Cmd::Book(BookCmd::Mine { engine, options, output, book, workers, nodes, rounds, leaves, eval_diff, multipv, max_ply, roots, black_only, white_only }) => {
            let cfg = engine_config(engine, options)?;
            let req = MineRequest {
                book_in: Some(output.clone()),
                raw_out: output,
                book_out: book,
                workers,
                roots,
                nodes,
                multipv,
                eval_diff,
                max_ply,
                leaves_per_round: leaves,
                rounds,
                black: !white_only,
                white: !black_only,
                ..Default::default()
            };
            let p = tools::mine(&req, &cfg, &cancel, |p| {
                eprint!("\rround {} | searched {} | queued {} | positions {} | errors {}   ", p.round, p.searched, p.queued, p.positions, p.errors);
            })?;
            eprintln!();
            println!("{} rounds, {} searches, {} positions in the book, {} errors", p.round, p.searched, p.positions, p.errors);
        }
        Cmd::Kifu(KifuCmd::Convert { inputs, out_dir, to, encoding, eval, force }) => {
            std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
            let req = KifuConvertRequest { inputs, out_dir, format: to, encoding, eval_style: eval, overwrite: force };
            let s = tools::convert_kifu_files(&req, &cancel, |_, _| {})?;
            for (p, e) in &s.failed {
                eprintln!("{p}: {e}");
            }
            println!("{} converted, {} failed", s.converted, s.failed.len());
            if !s.failed.is_empty() {
                return Err("some files could not be converted".into());
            }
        }
        Cmd::Position(PositionCmd::Convert { text, to, from }) => {
            let text = match text {
                Some(t) => t,
                None => {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().take(STDIN_LIMIT).read_to_string(&mut s).map_err(|e| e.to_string())?;
                    s
                }
            };
            let from = from.as_deref().map(position_format).transpose()?;
            println!("{}", tools::convert_position(&text, from, position_format(&to)?)?);
        }
        Cmd::Db(DbCmd::Convert { input, output, to, from, skip_without_eval, min_ply, force }) => {
            let req = DbConvertRequest { input, input_format: from, output, output_format: to, skip_without_eval, min_ply, overwrite: force };
            let s = tools::convert_db(&req, &cancel, |s| eprint!("\r{} games, {} positions   ", s.games, s.positions_written))?;
            eprintln!();
            println!("{} games, {} positions written, {} skipped", s.games, s.positions_written, s.skipped);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    // Messages are English unless a Japanese locale is requested.
    let lang = std::env::var("SAKIYOMI_LANG").or_else(|_| std::env::var("LANG")).unwrap_or_default();
    sakiyomi_app::i18n::set_lang(if lang.to_ascii_lowercase().starts_with("ja") { "ja" } else { "en" });
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn position_formats_parse() {
        assert_eq!(position_format("HCP").ok(), Some(PositionFormat::Hcp));
        assert!(position_format("nope").is_err());
        assert_eq!(side(Some("Gote")).ok(), Some(Some(1)));
    }
}
