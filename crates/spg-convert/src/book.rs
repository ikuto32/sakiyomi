//! Opening books ↔ the SPG position graph (§10.3, row "やねうら王定跡2016" and the rows
//! for hash-keyed and ShogiGUI books).
//!
//! * YaneuraOu `.db`: move → edge, value → evaluation (mover's view, unit 1, pawn = 100,
//!   unknown engine), depth → edge depth (or an ANLS with depth only when there is no
//!   value), ponder → ANLS PV, count → `src.yane.count` (never the weight, §10.3), the
//!   position's ply → `src.yane.ply`, comment → `comment`.
//! * ShogiGUI `.sbk` (judgement recorded here, §10.3 "要調査"): the move weight is a real
//!   selection weight → edge weight; the mark → `src.sbk.mark`; position statistics →
//!   `src.sbk.games/black_wins/white_wins`; position analyses → ANLS (context book,
//!   viewpoint unknown) with the KIF-text PV in `src.sbk.variation`.
//! * Gikou: frequency → `src.gikou.count`, wins → `src.gikou.wins`, score → evaluation.

use sakiyomi_book::{Book, BookFormat, BookMove, BookPosition, Mark, Move16Ser, PositionEval};
use spg::eval::{self, Unit};
use spg::model::*;
use spg::{Position, Spg};

use crate::{Lost, board_to_position, move_from_core, move_to_core, position_to_board};

fn prefix(fmt: BookFormat) -> &'static str {
    match fmt {
        BookFormat::YaneuraOuDb => "src.yane",
        BookFormat::Sbk => "src.sbk",
        BookFormat::Gikou => "src.gikou",
    }
}

fn source_name(fmt: BookFormat) -> &'static str {
    match fmt {
        BookFormat::YaneuraOuDb => "yaneuraou-db2016",
        BookFormat::Sbk => "shogigui-sbk",
        BookFormat::Gikou => "gikou-book",
    }
}

fn mark_name(m: Mark) -> Option<&'static str> {
    match m {
        Mark::None => None,
        Mark::Forced => Some("forced"),
        Mark::Good => Some("good"),
        Mark::Bad => Some("bad"),
        Mark::Blunder => Some("blunder"),
    }
}

fn mark_of(s: &str) -> Mark {
    match s {
        "forced" => Mark::Forced,
        "good" => Mark::Good,
        "bad" => Mark::Bad,
        "blunder" => Mark::Blunder,
        _ => Mark::None,
    }
}

fn clamp_eval(v: i32, annos: &mut Vec<Anno>) -> i32 {
    if !(-eval::NORMAL_MAX..=eval::NORMAL_MAX).contains(&v) {
        annos.push(Anno::str("src.range.eval", v.to_string()));
        v.clamp(-eval::NORMAL_MAX, eval::NORMAL_MAX)
    } else {
        v
    }
}

fn depth16(d: u32, annos: &mut Vec<Anno>, max: u16) -> u16 {
    if d > u32::from(max) {
        annos.push(Anno::str("src.range.depth", d.to_string()));
        max
    } else {
        d as u16
    }
}

/// One input of a book merge (§8.4): a source in SRCS.
pub struct BookSource<'a> {
    /// The source's name in SRCS (e.g. its file name).
    pub name: String,
    /// The format it was read from; `None` when it was read from SPG.
    pub format: Option<BookFormat>,
    pub book: &'a Book,
}

/// How several books were merged into one (§8.3, §8.4).
pub struct Merge<'a> {
    pub sources: &'a [BookSource<'a>],
    /// Write PROV. A writer may leave it out only when the user chose that explicitly; the
    /// merge itself is not such a choice (§8.4).
    pub prov: bool,
    /// The `merge.rule` text: how merged weights and values were decided, or, without PROV,
    /// that PROV was left out at the user's request.
    pub rule: String,
}

/// Convert a book read from `fmt` (§10.3).
pub fn book_to_spg(book: &Book, fmt: BookFormat) -> Spg {
    convert(book, fmt, None)
}

/// Convert the merge of `merge.sources` (already merged into `book`). Every source goes to
/// SRCS. Unless `merge.prov` is false, every position and edge gets a PROV entry for each
/// source whose book contains it, with that source's own values (§8.4). The effective rights
/// of each position and edge are the union of its sources' rights (§8.3), with or without PROV.
pub fn merged_books_to_spg(book: &Book, fmt: BookFormat, merge: &Merge) -> Spg {
    convert(book, fmt, Some(merge))
}

/// Rights bookkeeping for a merge (§8.3).
struct MergeRights {
    /// Rights of the whole file: the union of every source's rights.
    file: Vec<i64>,
    /// Effective rights of each source.
    source: Vec<Vec<i64>>,
    /// Every source has the file's rights, so no item needs its own.
    uniform: bool,
}

impl MergeRights {
    fn union<'s>(&self, sources: impl Iterator<Item = &'s usize>) -> Vec<i64> {
        let mut v: Vec<i64> = sources.flat_map(|&i| self.source[i].iter().copied()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

fn set_up_merge(spg: &mut Spg, merge: &Merge) -> MergeRights {
    let authors: Vec<Option<i64>> = merge
        .sources
        .iter()
        .map(|s| {
            let a = s.book.author.trim();
            (!a.is_empty()).then(|| spg.rights_index(Rights::author(a)) as i64)
        })
        .collect();
    let mut file: Vec<i64> = authors.iter().flatten().copied().collect();
    file.sort_unstable();
    file.dedup();
    // A source without rights information falls back to the file's rights (§8.3).
    let uniform = file.len() <= 1 && (file.is_empty() || authors.iter().all(Option::is_some));
    for &r in &file {
        spg.file_annos.push(Anno::int("rights", r));
    }
    let mut source = Vec::with_capacity(authors.len());
    for (s, a) in merge.sources.iter().zip(&authors) {
        let mut annos = Vec::new();
        match a {
            Some(r) if !uniform => {
                annos.push(Anno::int("rights", *r));
                source.push(vec![*r]);
            }
            _ => source.push(file.clone()),
        }
        spg.sources.push(Source { name: s.name.clone(), uri: None, annos });
    }
    spg.file_annos.push(Anno::str("merge.rule", merge.rule.clone()));
    MergeRights { file, source, uniform }
}

/// The values one source holds for an edge, as a PROV entry (same fields as EDGE). Values out
/// of a field's range are rounded and kept on the edge as `src.range.prov.<attr>` =
/// "<source>:<original>" (§8.6).
fn prov_of_move(source: u32, m: &BookMove, fmt: Option<BookFormat>, engine: u16, edge_annos: &mut Vec<Anno>) -> Prov {
    let mut note = |attr: &str, original: String| edge_annos.push(Anno::str(format!("src.range.prov.{attr}"), format!("{source}:{original}")));
    let weight = (fmt == Some(BookFormat::Sbk)).then(|| {
        u32::try_from(m.count).unwrap_or_else(|_| {
            note("weight", m.count.to_string());
            u32::MAX
        })
    });
    let eval = m.value.map(|v| {
        if !(-eval::NORMAL_MAX..=eval::NORMAL_MAX).contains(&v) {
            note("eval", v.to_string());
        }
        let depth = m.depth.map(|d| {
            // 0xFFFF means "none" in PROV: the largest depth is 65534.
            if d > u32::from(u16::MAX - 1) {
                note("depth", d.to_string());
            }
            d.min(u32::from(u16::MAX - 1)) as u16
        });
        EdgeEval { value: v.clamp(-eval::NORMAL_MAX, eval::NORMAL_MAX), engine, depth }
    });
    Prov { source, disabled: false, weight, eval, stats: None }
}

fn convert(book: &Book, fmt: BookFormat, merge: Option<&Merge>) -> Spg {
    let mut spg = Spg::default();
    let p = prefix(fmt);
    let engine = spg.unknown_engine(source_name(fmt), Unit::Cp, 100);
    let rights = match merge {
        Some(m) => Some(set_up_merge(&mut spg, m)),
        None => {
            if !book.author.trim().is_empty() {
                let r = spg.rights_index(Rights::author(book.author.trim()));
                spg.file_annos.push(Anno::int("rights", r as i64));
            }
            None
        }
    };
    // The engine of each source's values: the unknown engine of its format.
    let source_engines: Vec<u16> = merge.map_or(Vec::new(), |m| m.sources.iter().map(|s| s.format.map_or(engine, |f| spg.unknown_engine(source_name(f), Unit::Cp, 100))).collect());
    if !book.description.is_empty() {
        spg.file_annos.push(Anno::str("comment", book.description.clone()));
    }
    for (key, pos) in &book.positions {
        let Some(board) = Book::board(key) else { continue };
        let mut e = PositionEntry::new(board_to_position(&board));
        // The sources whose book contains this position.
        let contributors: Vec<(usize, &BookPosition)> = merge.map_or(Vec::new(), |m| m.sources.iter().enumerate().filter_map(|(i, s)| s.book.positions.get(key).map(|bp| (i, bp))).collect());
        let mut pos_rights = rights.as_ref().map(|r| r.file.clone()).unwrap_or_default();
        if let Some(r) = rights.as_ref().filter(|r| !r.uniform) {
            let u = r.union(contributors.iter().map(|(i, _)| i));
            if u != r.file {
                e.annos.extend(u.iter().map(|&x| Anno::int("rights", x)));
                pos_rights = u;
            }
        }
        if merge.is_some_and(|m| m.prov) {
            e.prov = contributors.iter().map(|&(i, _)| Prov { source: i as u32, ..Default::default() }).collect();
        }
        if let Some(ply) = pos.ply {
            e.annos.push(Anno::int(format!("{p}.ply"), i64::from(ply)));
        }
        if !pos.comment.is_empty() {
            e.annos.push(Anno::str("comment", pos.comment.clone()));
        }
        if !pos.by_ply.is_empty() {
            // §10.3: the sections of a position listed at several plies, as written.
            e.annos.push(Anno::str(format!("{p}.byply"), pos.by_ply.clone()));
        }
        if let Some(g) = pos.games {
            e.annos.push(Anno::int(format!("{p}.games"), i64::from(g)));
        }
        if let Some([b, w]) = pos.wins {
            e.annos.push(Anno::int(format!("{p}.black_wins"), i64::from(b)));
            e.annos.push(Anno::int(format!("{p}.white_wins"), i64::from(w)));
        }
        for pe in &pos.evals {
            let eng = match pe.engine.as_deref().filter(|n| !n.is_empty()) {
                Some(n) => spg.engine_index(n, "", Unit::Cp, 100),
                None => engine,
            };
            let mut a = Analysis::new(Context::Book, eng);
            a.eval = Some(clamp_eval(pe.value, &mut a.annos));
            a.viewpoint_unknown = true;
            a.depth = Some(depth16(pe.depth, &mut a.annos, u16::MAX));
            a.seldepth = Some(depth16(pe.seldepth, &mut a.annos, u16::MAX));
            a.nodes = Some(pe.nodes);
            if !pe.variation.is_empty() {
                a.annos.push(Anno::str(format!("{p}.variation"), pe.variation.clone()));
            }
            e.analyses.push(a);
        }
        for m in &pos.moves {
            let mut edge = Edge::new(move_from_core(m.mv.0));
            let mut book_anls: Option<Analysis> = None;
            match m.value {
                Some(v) => {
                    let value = clamp_eval(v, &mut edge.annos);
                    let depth = m.depth.map(|d| depth16(d, &mut edge.annos, u16::MAX - 1));
                    edge.eval = Some(EdgeEval { value, engine, depth });
                }
                None if m.depth.is_some() => {
                    // Depth without a value goes to an ANLS (§7.17).
                    let mut a = Analysis::new(Context::Book, engine);
                    a.depth = m.depth.map(|d| depth16(d, &mut a.annos, u16::MAX));
                    book_anls = Some(a);
                }
                None => {}
            }
            if let Some(pm) = m.ponder {
                let a = book_anls.get_or_insert_with(|| Analysis::new(Context::Book, engine));
                a.pv = Some(vec![move_from_core(pm.0)]);
            }
            edge.analyses.extend(book_anls);
            if fmt == BookFormat::Sbk {
                edge.weight = Some(u32::try_from(m.count).unwrap_or_else(|_| {
                    edge.annos.push(Anno::str("src.range.weight", m.count.to_string()));
                    u32::MAX
                }));
            } else {
                edge.annos.push(Anno::int(format!("{p}.count"), i64::try_from(m.count).unwrap_or(i64::MAX)));
            }
            if let Some(w) = m.wins {
                edge.annos.push(Anno::int(format!("{p}.wins"), i64::try_from(w).unwrap_or(i64::MAX)));
            }
            if let Some(name) = mark_name(m.mark) {
                edge.annos.push(Anno::str(format!("{p}.mark"), name));
            }
            if !m.comment.is_empty() {
                edge.annos.push(Anno::str("comment", m.comment.clone()));
            }
            if let Some(mg) = merge {
                let with_move: Vec<(usize, &BookMove)> = contributors.iter().filter_map(|&(i, bp)| bp.moves.iter().find(|x| x.mv.0 == m.mv.0).map(|x| (i, x))).collect();
                if let Some(r) = rights.as_ref().filter(|r| !r.uniform) {
                    let u = r.union(with_move.iter().map(|(i, _)| i));
                    if !u.is_empty() && u != pos_rights {
                        edge.annos.extend(u.iter().map(|&x| Anno::int("rights", x)));
                    }
                }
                if mg.prov {
                    let mut notes = Vec::new();
                    edge.prov = with_move.iter().map(|&(i, x)| prov_of_move(i as u32, x, mg.sources[i].format, source_engines[i], &mut notes)).collect();
                    edge.annos.extend(notes);
                }
            }
            e.edges.push(edge);
        }
        spg.positions.push(e);
    }
    spg
}

fn int_anno(annos: &[Anno], suffix: &str) -> Option<i64> {
    annos.iter().find(|a| a.key.starts_with("src.") && a.key.ends_with(suffix) && a.key.matches('.').count() == 2).and_then(|a| if let Value::Int(i) = a.value { Some(i) } else { None })
}

fn str_anno<'a>(annos: &'a [Anno], suffix: &str) -> Option<&'a str> {
    annos.iter().find(|a| a.key.starts_with("src.") && a.key.ends_with(suffix)).and_then(|a| if let Value::Str(s) = &a.value { Some(s.as_str()) } else { None })
}

/// The position graph as a book; records are ignored. Unusable data is reported.
pub fn spg_to_book(spg: &Spg) -> (Book, Lost) {
    let mut lost = Lost::default();
    let mut book = Book::default();
    if let Some(r) = spg.file_annos.iter().find(|a| a.key == "rights").and_then(|a| if let Value::Int(i) = a.value { spg.rights.get(i as usize) } else { None }) {
        book.author = r.name.clone();
    }
    book.description = spg.file_annos.iter().filter(|a| a.key == "comment").filter_map(|a| if let Value::Str(s) = &a.value { Some(s.clone()) } else { None }).collect::<Vec<_>>().join("\n");
    let usable = |engine: u16| spg.engines.get(usize::from(engine)).is_some_and(|e| matches!(e.unit, Unit::Cp | Unit::Raw));
    let prov: usize = spg.positions.iter().map(|p| p.prov.len() + p.edges.iter().map(|e| e.prov.len()).sum::<usize>()).sum();
    lost.add("provenance (PROV) entries", prov);
    let file_rights = spg.file_annos.iter().filter(|a| a.key == "rights").count();
    let item_rights: usize = spg.positions.iter().map(|p| p.annos.iter().chain(p.edges.iter().flat_map(|e| &e.annos)).filter(|a| a.key == "rights").count()).sum();
    lost.add("rights entries", file_rights.saturating_sub(1) + item_rights);
    for p in &spg.positions {
        let Some(board) = position_to_board(&p.position, 1) else {
            lost.add("positions the GUI rules cannot hold", 1);
            continue;
        };
        let mut bp = BookPosition {
            ply: int_anno(&p.annos, ".ply").and_then(|v| u32::try_from(v).ok()),
            comment: p.annos.iter().filter(|a| a.key == "comment").filter_map(|a| if let Value::Str(s) = &a.value { Some(s.as_str()) } else { None }).collect::<Vec<_>>().join("\n"),
            by_ply: str_anno(&p.annos, ".byply").unwrap_or("").to_owned(),
            games: int_anno(&p.annos, ".games").and_then(|v| u32::try_from(v).ok()),
            ..Default::default()
        };
        if let (Some(b), Some(w)) = (int_anno(&p.annos, ".black_wins"), int_anno(&p.annos, ".white_wins")) {
            bp.wins = Some([u32::try_from(b).unwrap_or(0), u32::try_from(w).unwrap_or(0)]);
        }
        for a in &p.analyses {
            if a.context != Context::Book || !usable(a.engine) {
                lost.add("position analyses", 1);
                continue;
            }
            bp.evals.push(PositionEval {
                value: a.eval.filter(|v| !eval::is_mate(*v)).unwrap_or(0),
                depth: a.depth.map_or(0, u32::from),
                seldepth: a.seldepth.map_or(0, u32::from),
                nodes: a.nodes.unwrap_or(0),
                variation: str_anno(&a.annos, ".variation").unwrap_or("").to_owned(),
                engine: spg.engines.get(usize::from(a.engine)).map(|e| e.name.clone()).filter(|n| !n.is_empty()),
            });
        }
        for e in &p.edges {
            if e.disabled {
                lost.add("negative (disabled) moves", 1);
                continue;
            }
            let Some(mv) = move_to_core(e.mv) else { continue };
            let book_anls = e.analyses.iter().find(|a| a.context == Context::Book);
            let value = e.eval.filter(|v| usable(v.engine)).map(|v| v.value).or_else(|| book_anls.filter(|a| usable(a.engine) && !a.viewpoint_unknown).and_then(|a| a.eval));
            if e.eval.is_some_and(|v| !usable(v.engine)) {
                lost.add("values in units other than points", 1);
            }
            // YaneuraOu-style books keep mates as large numbers.
            let value = value.map(|v| match eval::mate_parts(v) {
                Some((win, plies)) => {
                    let n = plies.unwrap_or(0).min(999) as i32;
                    if win { 32000 - n } else { -32000 + n }
                }
                None => v,
            });
            let count = int_anno(&e.annos, ".count").and_then(|v| u64::try_from(v).ok()).or(e.weight.map(u64::from)).or(e.stats.map(|s| s.games)).unwrap_or(1);
            let m = BookMove {
                mv: Move16Ser(mv),
                ponder: book_anls.and_then(|a| a.pv.as_ref()).and_then(|pv| pv.first()).and_then(|m| move_to_core(*m)).map(Move16Ser),
                value,
                depth: e.eval.and_then(|v| v.depth).or_else(|| book_anls.and_then(|a| a.depth)).map(u32::from),
                count,
                wins: int_anno(&e.annos, ".wins").and_then(|v| u64::try_from(v).ok()).or(e.stats.map(|s| s.wins)),
                mark: str_anno(&e.annos, ".mark").map_or(Mark::None, mark_of),
                comment: e.annos.iter().filter(|a| a.key == "comment").filter_map(|a| if let Value::Str(s) = &a.value { Some(s.as_str()) } else { None }).collect::<Vec<_>>().join("\n"),
            };
            bp.moves.push(m);
        }
        let _ = Position::startpos;
        book.positions.insert(Book::key(&board), bp);
    }
    (book, lost)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use sakiyomi_book::{ReadOptions, read, write, WriteOptions};

    const DB: &str = "#YANEURAOU-DB2016 1.00\nsfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1\n// the opening\n7g7f 3c3d 50 32 12 // main line\n2g2f none none 20 3\n5g5f 8c8d -10 none 1\nsfen lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 2\n3c3d 7f7e 40 18 9\n";

    #[test]
    fn yaneuraou_db_round_trip() {
        let (book, _) = read(BookFormat::YaneuraOuDb, DB.as_bytes(), &ReadOptions::default()).unwrap();
        let s = book_to_spg(&book, BookFormat::YaneuraOuDb);
        let start = &s.positions.iter().find(|p| p.position == Position::startpos()).unwrap();
        let e = start.edges.iter().find(|e| e.mv.to_usi() == "2g2f").unwrap();
        assert!(e.eval.is_none() && e.weight.is_none());
        assert_eq!(e.analyses[0].depth, Some(20), "depth without value goes to ANLS");
        assert!(e.annos.iter().any(|a| a.key == "src.yane.count" && a.value == Value::Int(3)));
        assert!(start.annos.iter().any(|a| a.key == "comment" && a.value == Value::Str("the opening".into())));
        let main = start.edges.iter().find(|e| e.mv.to_usi() == "7g7f").unwrap();
        assert!(main.annos.iter().any(|a| a.key == "comment" && a.value == Value::Str("main line".into())));
        let (bytes, _) = spg::write(&s, &spg::WriteOptions::default()).unwrap();
        let (back, _) = spg::read(&bytes, &spg::ReadOptions::default()).unwrap();
        let (b2, lost) = spg_to_book(&back);
        assert!(lost.is_empty(), "{}", lost.summary());
        let (out, _) = write(BookFormat::YaneuraOuDb, &b2, &WriteOptions::default()).unwrap();
        let (book2, _) = read(BookFormat::YaneuraOuDb, &out, &ReadOptions::default()).unwrap();
        assert_eq!(book2, book);
    }

    const DB2: &str = "#YANEURAOU-DB2016 1.00\nsfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1\n7g7f 3c3d 80 20 5\n6i7h none 10 70000 2\n";

    fn merged(prov: bool) -> (Spg, Book) {
        let (mut a, _) = read(BookFormat::YaneuraOuDb, DB.as_bytes(), &ReadOptions::default()).unwrap();
        let (mut b, _) = read(BookFormat::YaneuraOuDb, DB2.as_bytes(), &ReadOptions::default()).unwrap();
        a.author = "Alice".into();
        b.author = "Bob".into();
        let mut m = a.clone();
        sakiyomi_book::merge(&mut m, b.clone(), sakiyomi_book::MergePolicy::Sum);
        let sources = [
            BookSource { name: "a.db".into(), format: Some(BookFormat::YaneuraOuDb), book: &a },
            BookSource { name: "b.db".into(), format: Some(BookFormat::YaneuraOuDb), book: &b },
        ];
        let s = merged_books_to_spg(&m, BookFormat::YaneuraOuDb, &Merge { sources: &sources, prov, rule: "test rule".into() });
        (s, m)
    }

    #[test]
    fn merge_writes_prov_and_rights_by_source() {
        let (s, _) = merged(true);
        let (bytes, _) = spg::write(&s, &spg::WriteOptions::default()).unwrap();
        let (s, _) = spg::read(&bytes, &spg::ReadOptions::default()).unwrap();
        assert_eq!(s.sources.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["a.db", "b.db"]);
        assert!(s.file_annos.iter().any(|a| a.key == "merge.rule"));
        let rights_of = |annos: &[Anno]| -> Vec<String> {
            annos.iter().filter(|a| a.key == "rights").map(|a| if let Value::Int(i) = a.value { s.rights[i as usize].name.clone() } else { String::new() }).collect()
        };
        let start = s.positions.iter().find(|p| p.position == Position::startpos()).unwrap();
        assert_eq!(start.prov.iter().map(|p| p.source).collect::<Vec<_>>(), [0, 1], "both books contain the start");
        assert!(rights_of(&start.annos).is_empty(), "start has the file's rights (both authors)");
        // 7g7f is in both books: each source keeps its own value and depth.
        let e = start.edges.iter().find(|e| e.mv.to_usi() == "7g7f").unwrap();
        let vals: Vec<_> = e.prov.iter().map(|p| (p.source, p.eval.map(|v| (v.value, v.depth)))).collect();
        assert_eq!(vals, [(0, Some((50, Some(32)))), (1, Some((80, Some(20))))]);
        // 2g2f only in a.db, 6i7h only in b.db: their rights are that source's author alone.
        let e = start.edges.iter().find(|e| e.mv.to_usi() == "2g2f").unwrap();
        assert_eq!(e.prov.iter().map(|p| p.source).collect::<Vec<_>>(), [0]);
        assert_eq!(rights_of(&e.annos), ["Alice"]);
        let e = start.edges.iter().find(|e| e.mv.to_usi() == "6i7h").unwrap();
        assert_eq!(rights_of(&e.annos), ["Bob"]);
        // §8.6: a depth beyond the PROV field is rounded and its original kept on the edge.
        assert_eq!(e.prov[0].eval.unwrap().depth, Some(65534));
        assert!(e.annos.iter().any(|a| a.key == "src.range.prov.depth" && a.value == Value::Str("1:70000".into())));
        // The position after 7g7f is only in a.db.
        let after = s.positions.iter().find(|p| p.position != Position::startpos()).unwrap();
        assert_eq!(after.prov.iter().map(|p| p.source).collect::<Vec<_>>(), [0]);
        assert_eq!(rights_of(&after.annos), ["Alice"]);
        assert!(spg_to_book(&s).1.summary().contains("PROV"), "the book model reports what it cannot hold");
    }

    #[test]
    fn merge_without_prov_still_keeps_rights() {
        let (s, _) = merged(false);
        assert!(s.positions.iter().all(|p| p.prov.is_empty() && p.edges.iter().all(|e| e.prov.is_empty())));
        assert_eq!(s.sources.len(), 2);
        let after = s.positions.iter().find(|p| p.position != Position::startpos()).unwrap();
        assert_eq!(after.annos.iter().filter(|a| a.key == "rights").count(), 1);
        spg::write(&s, &spg::WriteOptions::default()).unwrap();
    }
}
