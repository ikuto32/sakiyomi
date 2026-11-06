//! Full games against the tanuki USI engine through the session API used by the GUI.
//! Run with: cargo test -p sakiyomi-app --test tanuki_game -- --ignored --test-threads 1 --nocapture
//! Uses Threads=1 and tiny budgets so it can run beside other workloads.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sakiyomi_app::session::{Event, GameConfig, Manager, PlayerConfig, Session, TimeControl};
use sakiyomi_kifu::format::{self, Format, TextEncoding};
use sakiyomi_kifu::{Limits, shogi_core};
use sakiyomi_usi::Registry;

/// The tanuki folder: `SAKIYOMI_TANUKI_DIR`, else `engines/tanuki-dr4` of the shogi_GPU_AI project.
fn tanuki() -> PathBuf {
    let root = std::env::var_os("SAKIYOMI_TANUKI_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../engines/tanuki-dr4"));
    root.join("tanuki-dr4-tournament-clang++-avx2.exe")
}

fn setup(tag: &str) -> (Arc<Registry>, Manager, String, Arc<Mutex<Vec<String>>>) {
    let dir = std::env::temp_dir().join(format!("sakiyomi-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let reg = Arc::new(Registry::load(dir.join("engines.json")));
    let cfg = reg.register(&tanuki().canonicalize().unwrap()).expect("register tanuki");
    assert!(cfg.usi_name.contains("tanuki"), "{}", cfg.usi_name);
    assert!(cfg.options.iter().any(|o| o.name == "Threads"));
    let mut values = BTreeMap::new();
    values.insert("Threads".to_owned(), "1".to_owned());
    values.insert("USI_Hash".to_owned(), "64".to_owned());
    values.insert("BookFile".to_owned(), "no_book".to_owned());
    values.insert("NetworkDelay".to_owned(), "50".to_owned());
    values.insert("NetworkDelay2".to_owned(), "100".to_owned());
    values.insert("MinimumThinkingTime".to_owned(), "1000".to_owned());
    let cfg = reg.update(&cfg.id, None, values).unwrap();
    // Persisted per engine.
    let again = Registry::load(dir.join("engines.json"));
    assert_eq!(again.get(&cfg.id).unwrap().values.get("Threads").map(String::as_str), Some("1"));

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let manager = Manager::new(Arc::new(move |e: Event| {
        let tag = match e {
            Event::Session(_) => "session",
            Event::Thinking(_) => "thinking",
            Event::Closed { .. } => "closed",
        };
        ev.lock().unwrap().push(tag.to_owned());
    }));
    (reg, manager, cfg.id, events)
}

fn engine(id: &str, nodes: Option<u64>) -> PlayerConfig {
    PlayerConfig { kind: "engine".into(), engine_id: Some(id.into()), nodes, ..Default::default() }
}

fn wait_over(s: &Session, limit: Duration, mut on_human: impl FnMut(&Session)) -> String {
    let start = Instant::now();
    loop {
        let v = s.view();
        let g = v.game.as_ref().expect("game");
        if g.over {
            return g.result.clone().unwrap_or_default();
        }
        if g.human_to_move && v.cursor == g.tip && !v.legal.is_empty() {
            on_human(s);
        }
        assert!(start.elapsed() < limit, "game did not finish in time: {}", v.status);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn check_record_roundtrip(s: &Session) {
    let l = Limits::default();
    let kif = s.export("kif", Some(sakiyomi_kifu::eval_comment::EvalStyle::ShogiGui)).unwrap();
    let rec = format::read(&kif, Some(Format::Kif), &l).expect("own KIF parses");
    let csa = s.export("csa", Some(sakiyomi_kifu::eval_comment::EvalStyle::Floodgate)).unwrap();
    let rec2 = format::read(&csa, Some(Format::Csa), &l).expect("own CSA parses");
    let v = s.view();
    let main_len = {
        let mut n = 0;
        let mut len = 1;
        while let Some(&c) = v.nodes[n].children.first() {
            n = c;
            len += 1;
        }
        len
    };
    assert_eq!(rec.mainline().len(), main_len);
    assert_eq!(rec2.mainline().len(), main_len);
    let sj = format::encode(&kif.replace("UTF-8", "Shift_JIS"), TextEncoding::ShiftJis);
    let rec3 = format::read(&format::decode(&sj), None, &l).unwrap();
    assert_eq!(rec3.mainline().len(), main_len);
    // Engine readings survive KIF and CSA export: both sides' evaluations are present.
    let evals = |r: &sakiyomi_kifu::GameRecord, side: u8| {
        r.nodes.iter().filter(|n| if side == 0 { n.analysis.black.is_some() } else { n.analysis.white.is_some() }).count()
    };
    assert!(evals(&rec, 0) > 0 || evals(&rec, 1) > 0);
    assert_eq!(evals(&rec, 0), evals(&rec2, 0));
}

#[test]
#[ignore]
fn engine_vs_engine_full_game() {
    let (reg, manager, id, events) = setup("eve");
    let s = manager.create(Some("EvE".into()));
    let cfg = GameConfig {
        players: [engine(&id, Some(1500)), engine(&id, Some(1500))],
        times: [TimeControl::default(); 2],
        start_sfen: Some(shogi_core::START_SFEN.into()),
        max_moves: 320,
    };
    s.new_game(cfg, &reg).expect("start game");
    let result = wait_over(&s, Duration::from_secs(600), |_| {});
    let v = s.view();
    println!("EvE result: {result}, plies: {}", v.nodes.len() - 1);
    assert!(v.nodes.len() > 20);
    // Every move carries the reading of the engine that played it (both sides).
    for (i, n) in v.nodes.iter().enumerate().skip(1).filter(|(_, n)| n.special.is_none()) {
        let mover_black = i % 2 == 1;
        let r = if mover_black { &n.analysis.black } else { &n.analysis.white };
        assert!(r.as_ref().is_some_and(|r| r.eval.is_some() && !r.pv.is_empty()), "node {i}");
    }
    check_record_roundtrip(&s);
    assert!(events.lock().unwrap().iter().any(|e| e == "thinking"));
    manager.close_all();
}

#[test]
#[ignore]
fn human_vs_engine_with_byoyomi_and_parallel_session() {
    let (reg, manager, id, _) = setup("hve");
    let a = manager.create(Some("HvE".into()));
    let b = manager.create(Some("Analysis".into()));
    // Session B analyses its own position with its own engine while A plays.
    b.import_text("position startpos moves 7g7f 3c3d 2g2f", None).unwrap();
    b.start_analysis(&reg, &id, 3, None).unwrap();

    let cfg = GameConfig {
        players: [PlayerConfig { kind: "human".into(), name: Some("Tester".into()), ..Default::default() }, engine(&id, None)],
        times: [TimeControl::default(), TimeControl { main_ms: 0, byoyomi_ms: 300, increment_ms: 0 }],
        start_sfen: Some(shogi_core::START_SFEN.into()),
        max_moves: 200,
    };
    a.new_game(cfg, &reg).expect("start game");
    let mut seed = 12345u64;
    let result = wait_over(&a, Duration::from_secs(600), |s| {
        let v = s.view();
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mv = &v.legal[(seed >> 33) as usize % v.legal.len()];
        s.make_move(mv).unwrap();
    });
    println!("HvE result: {result}, plies: {}", a.view().nodes.len() - 1);
    assert!(result.contains("後手の勝ち") || result.contains("千日手") || result.contains("持将棋"), "{result}");
    check_record_roundtrip(&a);

    // Session B kept analysing its own position, untouched by session A's game.
    let t = b.thinking_views();
    assert_eq!(t.len(), 1);
    assert!(t[0].lines.len() >= 2, "multipv lines: {}", t[0].lines.len());
    assert!(t[0].lines[0].pv.starts_with('△'), "{}", t[0].lines[0].pv);
    assert_eq!(b.view().analysis.as_ref().unwrap().node, 3);
    b.set_cursor(1).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let t = b.thinking_views();
    assert_eq!(t[0].node, 1);
    assert!(t[0].lines[0].pv.starts_with('△'));
    b.stop_analysis().unwrap();
    manager.close_all();
}

#[test]
#[ignore]
fn batch_analysis_fills_eval_graph() {
    let (reg, manager, id, _) = setup("batch");
    let s = manager.create(None);
    s.import_text("position startpos moves 7g7f 3c3d 2g2f 8c8d 2f2e 8d8e", None).unwrap();
    s.start_analysis(&reg, &id, 1, Some(200)).unwrap();
    let start = Instant::now();
    while s.view().analysis.is_some() {
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(50));
    }
    let v = s.view();
    assert!(v.nodes.iter().all(|n| n.analysis.analysis.is_some()));
    manager.close_all();
}

#[test]
#[ignore]
fn book_mining_with_tanuki() {
    use sakiyomi_app::tools::{MineRequest, mine};
    let (reg, _manager, id, _) = setup("mine");
    let engine = reg.get(&id).unwrap();
    let dir = std::env::temp_dir().join(format!("sakiyomi-mine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let req = MineRequest {
        raw_out: dir.join("raw.db"),
        book_out: Some(dir.join("book.db")),
        workers: 1,
        nodes: 3000,
        rounds: 2,
        leaves_per_round: 3,
        eval_diff: 50,
        ..Default::default()
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let p = mine(&req, &engine, &cancel, |p| println!("mining: {p:?}")).unwrap();
    assert_eq!(p.round, 2);
    assert!(p.searched >= 2 && p.errors == 0, "{p:?}");
    let (book, report) = sakiyomi_app::tools::read_book(&dir.join("book.db"), None, &Default::default()).unwrap();
    println!("mined book: {report}");
    assert!(book.positions.len() >= 2);
    assert_eq!(report.illegal_moves, 0);
    let _ = std::fs::remove_dir_all(&dir);
}
