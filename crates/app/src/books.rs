//! Opening books loaded for lookup (the GUI's book panel).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use sakiyomi_book::{Book, Mark, ReadOptions, Report};
use sakiyomi_kifu::notation;
use sakiyomi_kifu::record::parse_sfen;
use serde::Serialize;

use crate::i18n::kifu_error;
use crate::tools::read_book;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBookView {
    pub id: u32,
    pub path: String,
    pub name: String,
    pub positions: usize,
    pub report: Report,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMoveView {
    pub usi: String,
    /// KIF notation in the probed position, e.g. "７六歩(77)".
    pub label: String,
    /// From the side to move's point of view.
    pub value: Option<i32>,
    pub depth: Option<u32>,
    pub count: u64,
    /// Share of the total count, 0.0–1.0.
    pub share: f64,
    pub mark: Mark,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookProbeView {
    pub found: bool,
    /// The entry was found for the colour-flipped position.
    pub flipped: bool,
    pub moves: Vec<BookMoveView>,
    pub comment: String,
}

#[derive(Default)]
pub struct Books {
    next: AtomicU32,
    open: Mutex<BTreeMap<u32, (PathBuf, Arc<Book>)>>,
}

impl Books {
    /// Read a book (any supported format) and keep it for lookups.
    pub fn open(&self, path: &Path) -> Result<OpenBookView, String> {
        let (book, report) = if crate::spgio::SpgKind::of_path(path).is_some() {
            let (book, lost) = crate::spgio::read_book(path)?;
            let report = Report { positions: book.positions.len(), lost_fields: lost.0.into_iter().map(|(w, n)| format!("{w} ×{n}")).collect(), ..Default::default() };
            (book, report)
        } else {
            read_book(path, None, &ReadOptions::default())?
        };
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let view = OpenBookView {
            id,
            path: path.display().to_string(),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            positions: book.positions.len(),
            report,
        };
        self.open.lock().unwrap_or_else(|e| e.into_inner()).insert(id, (path.to_owned(), Arc::new(book)));
        Ok(view)
    }

    pub fn close(&self, id: u32) {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// Book moves for a position given as SFEN, best first.
    pub fn probe(&self, id: u32, sfen: &str) -> Result<BookProbeView, String> {
        let book = self.open.lock().unwrap_or_else(|e| e.into_inner()).get(&id).map(|(_, b)| b.clone());
        let Some(book) = book else { return Ok(BookProbeView::default()) };
        let board = parse_sfen(sfen).map_err(|e| kifu_error(&e))?;
        let Some((pos, flipped)) = sakiyomi_book::probe(&book, &board, true) else { return Ok(BookProbeView::default()) };
        let total: u64 = pos.moves.iter().map(|m| m.count).sum();
        let legal = board.legal_moves();
        let mut moves: Vec<BookMoveView> = pos
            .moves
            .iter()
            .filter_map(|m| {
                let mv = if flipped { sakiyomi_book::flip_move(m.mv.0)? } else { m.mv.0 };
                legal.contains(&mv).then(|| BookMoveView {
                    usi: mv.to_usi(),
                    label: notation::kif_move(&board, mv, None),
                    value: m.value,
                    depth: m.depth,
                    count: m.count,
                    share: if total > 0 { m.count as f64 / total as f64 } else { 0.0 },
                    mark: m.mark,
                })
            })
            .collect();
        moves.sort_by(|a, b| b.value.unwrap_or(i32::MIN).cmp(&a.value.unwrap_or(i32::MIN)).then(b.count.cmp(&a.count)));
        Ok(BookProbeView { found: true, flipped, moves, comment: pos.comment.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_probe() {
        let dir = std::env::temp_dir().join(format!("sakiyomi-books-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.db");
        std::fs::write(
            &path,
            "#YANEURAOU-DB2016 1.00\nsfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1\n7g7f 3c3d 50 20 3\n2g2f none 30 18 1\n",
        )
        .unwrap();
        let books = Books::default();
        let v = books.open(&path).unwrap();
        assert_eq!(v.positions, 1);
        let p = books.probe(v.id, sakiyomi_kifu::shogi_core::START_SFEN).unwrap();
        assert!(p.found && !p.flipped);
        assert_eq!(p.moves.iter().map(|m| m.usi.as_str()).collect::<Vec<_>>(), ["7g7f", "2g2f"]);
        assert_eq!(p.moves[0].label, "７六歩(77)");
        assert!((p.moves[0].share - 0.75).abs() < 1e-9);
        // Unknown positions and closed books are simply empty.
        let other = books.probe(v.id, "lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 2").unwrap();
        assert!(!other.found);
        books.close(v.id);
        assert!(!books.probe(v.id, sakiyomi_kifu::shogi_core::START_SFEN).unwrap().found);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
