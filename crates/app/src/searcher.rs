//! A USI engine as a book-mining [`Searcher`]: `setoption MultiPV`, `position`, `go nodes`,
//! collecting the last `info … multipv k` line of each PV until `bestmove`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use sakiyomi_book::miner::{SearchLine, Searcher};
use sakiyomi_kifu::format::usi_position_from;
use sakiyomi_kifu::shogi_core::Move16;
use sakiyomi_kifu::shogi_core::rules::Board;
use sakiyomi_usi::{EngineConfig, Score, UsiProcess, parse_info, start_engine};

use crate::i18n::trf;

/// A search may take long with large node budgets; give up only on a silent engine.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(600);

pub struct UsiSearcher {
    proc: UsiProcess,
    rx: Receiver<String>,
    multipv: usize,
    /// Checked while searching: a set flag stops the running search at once.
    cancel: Arc<AtomicBool>,
}

impl UsiSearcher {
    pub fn start(engine: &EngineConfig, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        let overrides = [("USI_Ponder".to_owned(), "false".to_owned())];
        let (mut proc, _) = start_engine(Path::new(&engine.path), &engine.effective_values(&overrides), Duration::from_secs(180))
            .map_err(|e| trf("エンジンの探索に失敗しました: {err}", &[("err", &e)]))?;
        let rx = proc.take_receiver().ok_or_else(|| "engine output unavailable".to_owned())?;
        Ok(UsiSearcher { proc, rx, multipv: 0, cancel })
    }
}

/// YaneuraOu book convention: mate in n = ±(32000 − n), cp clamped to ±30000.
fn book_value(s: &Score) -> i32 {
    match *s {
        Score::Cp(cp) => cp.clamp(-30000, 30000),
        Score::Mate(n) if n.abs() >= 999 => if n > 0 { 31999 } else { -31999 },
        Score::Mate(n) => if n >= 0 { 32000 - n } else { -32000 - n },
    }
}

impl Searcher for UsiSearcher {
    fn search(&mut self, root: &Board, moves: &[Move16], multipv: usize, nodes: u64) -> Result<Vec<SearchLine>, String> {
        let err = |e: &dyn std::fmt::Display| trf("エンジンの探索に失敗しました: {err}", &[("err", e)]);
        if multipv != self.multipv {
            self.proc.set_option("MultiPV", &multipv.to_string()).map_err(|e| err(&e))?;
            self.multipv = multipv;
        }
        let mut start = *root;
        start.move_number = start.move_number.max(1);
        self.proc.send(&usi_position_from(&start.to_sfen(), moves)).map_err(|e| err(&e))?;
        self.proc.send(&format!("go nodes {nodes}")).map_err(|e| err(&e))?;
        let mut lines: BTreeMap<u32, SearchLine> = BTreeMap::new();
        let mut last = Instant::now();
        let mut stopped = false;
        loop {
            if !stopped && self.cancel.load(Ordering::Relaxed) {
                let _ = self.proc.send("stop");
                stopped = true;
            }
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    last = Instant::now();
                    if line.starts_with("bestmove") {
                        break;
                    }
                    let Some(info) = parse_info(&line) else { continue };
                    if info.bound.is_some() {
                        continue; // fail-high/low lines are not final scores
                    }
                    let (Some(score), Some(first)) = (info.score, info.pv.first()) else { continue };
                    let Ok(mv) = Move16::from_usi(first) else { continue };
                    lines.insert(info.multipv.unwrap_or(1), SearchLine { mv, value: book_value(&score) });
                }
                Err(RecvTimeoutError::Timeout) if last.elapsed() < SILENCE_TIMEOUT => {}
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.proc.send("stop");
                    return Err(err(&"timeout"));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(err(&"the engine exited")),
            }
        }
        if stopped {
            // A stopped search is shallower than asked for; do not store it.
            return Err(err(&"cancelled"));
        }
        let mut out: Vec<SearchLine> = lines.into_values().collect();
        out.sort_by_key(|l| std::cmp::Reverse(l.value));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mate_scores_map_like_bookminer() {
        assert_eq!(book_value(&Score::Cp(123)), 123);
        assert_eq!(book_value(&Score::Cp(99999)), 30000);
        assert_eq!(book_value(&Score::Mate(5)), 31995);
        assert_eq!(book_value(&Score::Mate(-4)), -31996);
        assert_eq!(book_value(&Score::Mate(999)), 31999);
    }
}
