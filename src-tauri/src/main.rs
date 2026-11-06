// Release builds are GUI-subsystem executables (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Tauri adapter: every command forwards to `sakiyomi_app`. Long work (conversions, mining,
//! opening large books) runs as a background job reported through `shogi://job` events.

mod monitor;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sakiyomi_app::books::{BookProbeView, Books};
use sakiyomi_app::jobs::{JobView, Jobs};
use sakiyomi_app::session::{Event, GameConfig, Manager, SessionView, ThinkingView};
use sakiyomi_app::tools::{self, BookConvertRequest, DbConvertRequest, KifuConvertRequest, MineRequest, PositionFormat};
use sakiyomi_kifu::format::Format;
use sakiyomi_usi::{EngineConfig, Registry};
use tauri::{Emitter, Manager as _, State};

struct AppState {
    manager: Arc<Manager>,
    registry: Arc<Registry>,
    jobs: Jobs,
    books: Arc<Books>,
}

type Res<T> = Result<T, String>;

fn json<T: serde::Serialize>(v: T) -> Res<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- engines

#[tauri::command(async)]
fn list_engines(state: State<AppState>) -> Vec<EngineConfig> {
    state.registry.list()
}

#[tauri::command(async)]
fn register_engine(state: State<AppState>, path: String) -> Res<EngineConfig> {
    state.registry.register(&PathBuf::from(path))
}

#[tauri::command(async)]
fn refresh_engine(state: State<AppState>, id: String) -> Res<EngineConfig> {
    state.registry.refresh(&id)
}

#[tauri::command(async)]
fn update_engine(state: State<AppState>, id: String, name: Option<String>, values: BTreeMap<String, String>) -> Res<EngineConfig> {
    state.registry.update(&id, name, values)
}

#[tauri::command(async)]
fn remove_engine(state: State<AppState>, id: String) -> Res<()> {
    state.registry.remove(&id)
}

/// UI language for backend messages ("ja", "en", …).
#[tauri::command]
fn set_language(lang: String) {
    sakiyomi_app::i18n::set_lang(&lang);
}

// ---------------------------------------------------------------- sessions

#[tauri::command(async)]
fn create_session(state: State<AppState>, title: Option<String>) -> SessionView {
    state.manager.create(title).view()
}

#[tauri::command(async)]
fn close_session(state: State<AppState>, id: u32) {
    state.manager.close(id)
}

#[tauri::command(async)]
fn list_sessions(state: State<AppState>) -> Vec<SessionView> {
    state.manager.list()
}

#[tauri::command(async)]
fn thinking(state: State<AppState>, id: u32) -> Res<Vec<ThinkingView>> {
    Ok(state.manager.get(id)?.thinking_views())
}

#[tauri::command(async)]
fn set_title(state: State<AppState>, id: u32, title: String) -> Res<()> {
    state.manager.get(id)?.set_title(&title)
}

#[tauri::command(async)]
fn set_cursor(state: State<AppState>, id: u32, node: usize) -> Res<()> {
    state.manager.get(id)?.set_cursor(node)
}

#[tauri::command(async)]
fn make_move(state: State<AppState>, id: u32, usi: String) -> Res<()> {
    state.manager.get(id)?.make_move(&usi)
}

#[tauri::command(async)]
fn new_game(state: State<AppState>, id: u32, config: GameConfig) -> Res<()> {
    state.manager.get(id)?.new_game(config, &state.registry)
}

#[tauri::command(async)]
fn stop_game(state: State<AppState>, id: u32) -> Res<()> {
    state.manager.get(id)?.stop_game()
}

#[tauri::command(async)]
fn resign(state: State<AppState>, id: u32) -> Res<()> {
    state.manager.get(id)?.resign()
}

#[tauri::command(async)]
fn move_now(state: State<AppState>, id: u32) -> Res<()> {
    state.manager.get(id)?.move_now()
}

#[tauri::command(async)]
fn start_analysis(state: State<AppState>, id: u32, engine_id: String, multipv: u32, batch_ms: Option<u64>) -> Res<()> {
    state.manager.get(id)?.start_analysis(&state.registry, &engine_id, multipv, batch_ms)
}

#[tauri::command(async)]
fn stop_analysis(state: State<AppState>, id: u32) -> Res<()> {
    state.manager.get(id)?.stop_analysis()
}

/// Records opened and what the GUI could not show.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadResult {
    opened: usize,
    notice: Option<String>,
}

/// Further records of a multi-record file open in new sessions, up to this many.
const MAX_EXTRA_SESSIONS: usize = 32;

#[tauri::command(async)]
fn load_file(state: State<AppState>, id: u32, path: String) -> Res<LoadResult> {
    let path = PathBuf::from(path);
    let notes = state.manager.get(id)?.load_file(&path)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let total = notes.more.len() + 1;
    for (k, rec) in notes.more.into_iter().take(MAX_EXTRA_SESSIONS).enumerate() {
        let s = state.manager.create(Some(format!("{stem} ({})", k + 2)));
        s.load_record(rec, None)?;
    }
    let opened = total.min(MAX_EXTRA_SESSIONS + 1);
    let mut notice = Vec::new();
    if total > opened {
        notice.push(sakiyomi_app::i18n::trf("{total}局のうち{opened}局を開きました", &[("total", &total), ("opened", &opened)]));
    }
    if !notes.lost.is_empty() {
        notice.push(sakiyomi_app::i18n::trf("表示できない情報があります（保存すると失われます）: {what}", &[("what", &notes.lost)]));
    }
    notice.extend(notes.errors);
    Ok(LoadResult { opened, notice: (!notice.is_empty()).then(|| notice.join("\n")) })
}

fn format_of(name: &str) -> Res<Format> {
    Format::from_name(name).ok_or_else(|| sakiyomi_app::i18n::tr("未対応の形式です").to_owned())
}

/// `encoding`: utf8 | utf8bom | sjis. `eval_style`: shogigui | shogihome | floodgate | none.
#[tauri::command(async)]
fn save_file(state: State<AppState>, id: u32, path: String, format: String, encoding: String, eval_style: Option<String>) -> Res<()> {
    match format.as_str() {
        "spg" => return state.manager.get(id)?.save_spg(&PathBuf::from(path), false),
        "spgt" => return state.manager.get(id)?.save_spg(&PathBuf::from(path), true),
        _ => {}
    }
    let style = eval_style.as_deref().and_then(tools::eval_style);
    state.manager.get(id)?.save_file(&PathBuf::from(path), format_of(&format)?, tools::text_encoding(&encoding), style)
}

#[tauri::command(async)]
fn import_text(state: State<AppState>, id: u32, text: String) -> Res<()> {
    state.manager.get(id)?.import_text(&text, None)
}

#[tauri::command(async)]
fn export_text(state: State<AppState>, id: u32, what: String, eval_style: Option<String>) -> Res<String> {
    let style = eval_style.as_deref().and_then(tools::eval_style);
    state.manager.get(id)?.export(&what, style)
}

#[tauri::command(async)]
fn set_position(state: State<AppState>, id: u32, sfen: String) -> Res<()> {
    state.manager.get(id)?.set_position(&sfen)
}

#[tauri::command(async)]
fn delete_node(state: State<AppState>, id: u32, node: usize) -> Res<()> {
    state.manager.get(id)?.delete_node(node)
}

#[tauri::command(async)]
fn promote_variation(state: State<AppState>, id: u32, node: usize) -> Res<()> {
    state.manager.get(id)?.promote_variation(node)
}

#[tauri::command(async)]
fn set_comment(state: State<AppState>, id: u32, node: usize, text: String) -> Res<()> {
    state.manager.get(id)?.set_comment(node, &text)
}

#[tauri::command(async)]
fn set_header(state: State<AppState>, id: u32, key: String, value: String) -> Res<()> {
    state.manager.get(id)?.set_header(&key, &value)
}

// ---------------------------------------------------------------- tools (background jobs)

#[tauri::command(async)]
fn job_convert_books(state: State<AppState>, req: BookConvertRequest) -> u32 {
    let title = req.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    state.jobs.spawn("convertBooks", title, move |ctx| {
        ctx.progress(None, "");
        json(tools::convert_books(&req)?)
    })
}

#[tauri::command(async)]
fn job_convert_kifu(state: State<AppState>, req: KifuConvertRequest) -> u32 {
    let title = req.out_dir.display().to_string();
    state.jobs.spawn("convertKifu", title, move |ctx| {
        let s = tools::convert_kifu_files(&req, &ctx.cancel, |i, n| ctx.progress(Some(i as f64 / n.max(1) as f64), format!("{i}/{n}")))?;
        json(s)
    })
}

#[tauri::command(async)]
fn job_convert_db(state: State<AppState>, req: DbConvertRequest) -> u32 {
    let title = req.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    state.jobs.spawn("convertDb", title, move |ctx| {
        let s = tools::convert_db(&req, &ctx.cancel, |s| ctx.progress(None, format!("{} / {}", s.games, s.positions_written)))?;
        json(s)
    })
}

#[tauri::command(async)]
fn job_mine(state: State<AppState>, engine_id: String, req: MineRequest) -> Res<u32> {
    let engine = state.registry.get(&engine_id).ok_or_else(|| sakiyomi_app::i18n::tr("エンジンが見つかりません").to_owned())?;
    let title = req.raw_out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(state.jobs.spawn("mine", title, move |ctx| {
        let total = req.rounds;
        let p = tools::mine(&req, &engine, &ctx.cancel, |p| {
            let frac = (total > 0).then(|| p.round as f64 / total as f64);
            ctx.progress(frac, serde_json::to_string(p).unwrap_or_default());
        })?;
        json(p)
    }))
}

#[tauri::command(async)]
fn job_book_stats(state: State<AppState>, path: String) -> u32 {
    let title = PathBuf::from(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    state.jobs.spawn("bookStats", title, move |ctx| {
        ctx.progress(None, "");
        let (book, report) = tools::read_book(&PathBuf::from(&path), None, &Default::default())?;
        json(serde_json::json!({ "report": report, "stats": sakiyomi_book::stats(&book) }))
    })
}

#[tauri::command(async)]
fn job_open_book(state: State<AppState>, path: String) -> u32 {
    let books = state.books.clone();
    let title = PathBuf::from(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    state.jobs.spawn("openBook", title, move |ctx| {
        ctx.progress(None, "");
        json(books.open(&PathBuf::from(&path))?)
    })
}

#[tauri::command(async)]
fn close_book(state: State<AppState>, id: u32) {
    state.books.close(id)
}

#[tauri::command(async)]
fn probe_book(state: State<AppState>, id: u32, sfen: String) -> Res<BookProbeView> {
    state.books.probe(id, &sfen)
}

#[tauri::command(async)]
fn convert_position(text: String, from: Option<PositionFormat>, to: PositionFormat) -> Res<String> {
    tools::convert_position(&text, from, to)
}

#[tauri::command(async)]
fn list_jobs(state: State<AppState>) -> Vec<JobView> {
    state.jobs.list()
}

#[tauri::command(async)]
fn cancel_job(state: State<AppState>, id: u32) {
    state.jobs.cancel(id)
}

#[tauri::command(async)]
fn clear_jobs(state: State<AppState>) {
    state.jobs.clear_finished()
}

// ---------------------------------------------------------------- startup

/// The app was called "Shogi GUI" (identifier dev.shogigpu.shogigui) before it became
/// Sakiyomi. Copy its engine list and WebView data (layouts, settings) once, so nothing is lost.
fn migrate_from_old_identifier() {
    const OLD: &str = "dev.shogigpu.shogigui";
    const NEW: &str = "dev.shogigpu.sakiyomi";
    for var in ["APPDATA", "LOCALAPPDATA"] {
        let Some(base) = std::env::var_os(var).map(PathBuf::from) else { continue };
        let (old, new) = (base.join(OLD), base.join(NEW));
        if old.is_dir() && !new.exists() {
            let _ = copy_dir(&old, &new, 0);
        }
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path, depth: u32) -> std::io::Result<()> {
    if depth > 16 {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            copy_dir(&entry.path(), &target, depth + 1)?;
        } else {
            // Locked files (e.g. WebView caches of a running instance) are skipped.
            let _ = std::fs::copy(entry.path(), &target);
        }
    }
    Ok(())
}

fn main() {
    migrate_from_old_identifier();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let dir = app.path().app_config_dir().unwrap_or_else(|_| PathBuf::from("."));
            let registry = Arc::new(Registry::load(dir.join("engines.json")));
            let handle = app.handle().clone();
            let manager = Arc::new(Manager::new(Arc::new(move |e: Event| {
                let _ = match &e {
                    Event::Session(v) => handle.emit("shogi://session", v),
                    Event::Thinking(t) => handle.emit("shogi://thinking", t),
                    Event::Closed { id } => handle.emit("shogi://closed", id),
                };
            })));
            let job_handle = app.handle().clone();
            let jobs = Jobs::new(Arc::new(move |v: &JobView| {
                let _ = job_handle.emit("shogi://job", v);
            }));
            let stats_handle = app.handle().clone();
            monitor::spawn(manager.clone(), move |s| {
                let _ = stats_handle.emit("shogi://system", s);
            });
            app.manage(AppState { manager, registry, jobs, books: Arc::new(Books::default()) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_language,
            list_engines,
            register_engine,
            refresh_engine,
            update_engine,
            remove_engine,
            create_session,
            close_session,
            list_sessions,
            thinking,
            set_title,
            set_cursor,
            make_move,
            new_game,
            stop_game,
            resign,
            move_now,
            start_analysis,
            stop_analysis,
            load_file,
            save_file,
            import_text,
            export_text,
            set_position,
            delete_node,
            promote_variation,
            set_comment,
            set_header,
            job_convert_books,
            job_convert_kifu,
            job_convert_db,
            job_mine,
            job_book_stats,
            job_open_book,
            close_book,
            probe_book,
            convert_position,
            list_jobs,
            cancel_job,
            clear_jobs,
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            eprintln!("failed to start Sakiyomi: {e}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Make sure no engine process outlives the GUI: stop jobs (mining engines get
            // `stop`), give them a moment to wind down, then close sessions.
            let state = handle.state::<AppState>();
            state.jobs.cancel_all();
            state.jobs.wait_idle(Duration::from_secs(5));
            state.manager.close_all();
        }
    });
}
