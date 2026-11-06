//! SPG, the application's standard format: reading and writing `.spg`, `.spgt` and `.spgm`
//! for sessions, tools and the book panel.

use std::path::Path;

use sakiyomi_book::{Book, BookFormat, MergePolicy};
use sakiyomi_kifu::record::GameRecord;
use sakiyomi_spg::record::Origin;
use sakiyomi_spg::{BookSource, Lost, Merge, book_to_spg, merged_books_to_spg, records_to_spg, spg_to_book, spg_to_records};
use serde::Serialize;
use spg::Spg;

use crate::i18n::{tr, trf};
use crate::tools::{MAX_BOOK_BYTES, atomic_write, check_output};

/// The SPG flavour of a path or format name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpgKind {
    Binary,
    Text,
    Manifest,
}

impl SpgKind {
    pub fn from_name(name: &str) -> Option<SpgKind> {
        match name.to_ascii_lowercase().trim_start_matches('.') {
            "spg" => Some(SpgKind::Binary),
            "spgt" => Some(SpgKind::Text),
            "spgm" => Some(SpgKind::Manifest),
            _ => None,
        }
    }

    pub fn of_path(path: &Path) -> Option<SpgKind> {
        let name = path.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".spgm.bak") {
            return Some(SpgKind::Manifest);
        }
        path.extension().and_then(|e| e.to_str()).and_then(SpgKind::from_name)
    }
}

pub fn spg_error(e: impl std::fmt::Display) -> String {
    trf("SPGファイルを読み書きできません: {err}", &[("err", &e)])
}

pub fn read(path: &Path) -> Result<Spg, String> {
    sakiyomi_spg::read_path(path, MAX_BOOK_BYTES, &spg::ReadOptions::default()).map_err(spg_error)
}

/// Write `.spg` / `.spgt` atomically, or a `.spgm` file set next to `path`.
pub fn write(path: &Path, data: &Spg, kind: SpgKind, overwrite: bool) -> Result<(), String> {
    match kind {
        SpgKind::Manifest => {
            check_output(path, overwrite)?;
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let name = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".spgm")).ok_or_else(|| tr("マニフェストは <名前>.spgm です").to_owned())?;
            let split = data.positions.len() > 1_000_000;
            spg::manifest::write_set(dir, name, data, &spg::manifest::SetOptions { split, ..Default::default() }).map_err(spg_error)?;
            Ok(())
        }
        _ => {
            let bytes = sakiyomi_spg::to_bytes(data, kind == SpgKind::Text).map_err(spg_error)?;
            atomic_write(path, &bytes, overwrite)
        }
    }
}

/// What opening an SPG file as game records left out.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordsRead {
    #[serde(skip)]
    pub records: Vec<GameRecord>,
    /// Data the GUI cannot show (and will not save back).
    pub lost: String,
    /// Records that could not be opened.
    pub errors: Vec<String>,
}

pub fn read_records(path: &Path) -> Result<RecordsRead, String> {
    let data = read(path)?;
    if data.records.is_empty() {
        return Err(tr("棋譜が含まれていません").to_owned());
    }
    let (records, lost, errors) = spg_to_records(&data);
    if records.is_empty() {
        return Err(errors.first().cloned().unwrap_or_else(|| tr("棋譜が含まれていません").to_owned()));
    }
    Ok(RecordsRead { records, lost: lost.summary(), errors })
}

pub fn records_to_bytes(recs: &[GameRecord], text: bool) -> Result<Vec<u8>, String> {
    let data = records_to_spg(recs, &Origin { format: "sakiyomi".into(), raw: None });
    sakiyomi_spg::to_bytes(&data, text).map_err(spg_error)
}

/// A book read from SPG, with what the book model leaves out.
pub fn read_book(path: &Path) -> Result<(Book, Lost), String> {
    Ok(spg_to_book(&read(path)?))
}

/// How a book conversion merged its inputs, for SPG output (§8.4).
pub struct BookMerge<'a> {
    /// Every input, in merge order; a single input is not a merge.
    pub sources: &'a [BookSource<'a>],
    /// The merged entries' rule: how weights and values were decided.
    pub rule: String,
    /// The user's explicit choice to leave PROV out. Asking for a merge is not that choice.
    pub no_prov: bool,
}

/// The `merge.rule` text for `sakiyomi_book::merge` with `policy`, then the filters.
pub fn merge_rule(policy: MergePolicy, after: &[String]) -> String {
    let how = match policy {
        MergePolicy::Sum => {
            "for a position in several sources, moves are combined: counts add up; value, depth and ponder come from the last source that has them; position evaluations are kept from all sources; the first non-empty comment and the smallest ply are kept"
        }
        MergePolicy::KeepFirst => "for a position in several sources, the first source's entry is kept",
        MergePolicy::PreferSecond => "for a position in several sources, the last source's entry is kept",
    };
    let mut rule = format!("Sakiyomi book merge: sources merged in SRCS order; {how}");
    for a in after {
        rule.push_str("; then ");
        rule.push_str(a);
    }
    rule
}

/// SPG for a book; `source` names the format the data came from (§10.3 mappings).
pub fn book_data(book: &Book, source: BookFormat, merge: Option<BookMerge>) -> Spg {
    match merge.filter(|m| m.sources.len() > 1) {
        None => book_to_spg(book, source),
        Some(m) => {
            let mut rule = m.rule;
            if m.no_prov {
                // §8.4: PROV is left out only on the user's explicit option, recorded here.
                rule.push_str("; PROV left out at the user's request (option no-prov)");
            }
            merged_books_to_spg(book, source, &Merge { sources: m.sources, prov: !m.no_prov, rule })
        }
    }
}
