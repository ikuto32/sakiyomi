//! YaneuraOu book DB (`#YANEURAOU-DB2016 1.00`).
//!
//! ```text
//! #YANEURAOU-DB2016 1.00
//! sfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1
//! 7g7f 3c3d 40 20 10
//! ```
//! Move lines are `move ponder [value [depth [count]]]` (value from the side to move's view;
//! defaults 0, 0, 1; `none` = missing). Lines starting with `#` or `//` are comments. Positions
//! are written sorted by their `sfen …` line in byte order, which `BookOnTheFly` requires.

use sakiyomi_kifu::shogi_core::Move16;

use crate::{Book, BookError, BookLimits, BookMove, BookPosition, Move16Ser, Report, Result};

pub const HEADER: &str = "#YANEURAOU-DB2016 1.00";

fn parse_opt_i32(t: Option<&str>) -> Option<i32> {
    t.filter(|s| *s != "none").and_then(|s| s.parse::<i64>().ok()).map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// A line without its `//` comment, and the comment.
fn split_comment(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once("//") {
        Some((code, c)) => (code.trim(), Some(c.trim())),
        None => (raw.trim(), None),
    }
}

/// Append `text` as a new line of `to` (empty comments add nothing unless `keep_empty`).
fn add_line(to: &mut String, text: &str, keep_empty: bool) {
    if text.is_empty() && !keep_empty {
        return;
    }
    if !to.is_empty() {
        to.push('\n');
    }
    to.push_str(text);
}

/// Read a YaneuraOu book. Text after `//` becomes the comment of the move on that line, of the
/// position (on an `sfen` line, or a comment line inside its section), or of the book (before
/// the first position). A position listed in several sections is merged into one entry, and
/// the original text of those sections is kept in [`BookPosition::by_ply`].
pub fn read(bytes: &[u8], limits: &BookLimits) -> Result<(Book, Report)> {
    let text = std::str::from_utf8(bytes).map_err(|_| BookError::Corrupt("not UTF-8/ASCII".into()))?;
    let mut book = Book::default();
    let mut report = Report::default();
    let mut current: Option<String> = None;
    let lines: Vec<&str> = text.lines().collect();
    // (position, first line, end line) of every section.
    let mut sections: Vec<(String, usize, usize)> = Vec::new();
    for (no, raw) in lines.iter().enumerate() {
        if raw.len() > limits.max_line_bytes {
            return Err(BookError::TooLarge { what: "line", limit: limits.max_line_bytes });
        }
        let (line, comment) = split_comment(raw);
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            if let Some(c) = comment {
                match current.as_ref().and_then(|k| book.positions.get_mut(k)) {
                    Some(p) => add_line(&mut p.comment, c, false),
                    None if sections.is_empty() => add_line(&mut book.description, c, false),
                    None => {}
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("sfen ") {
            if let Some(s) = sections.last_mut() {
                s.2 = s.2.min(no);
            }
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() < 3 {
                report.invalid_positions += 1;
                current = None;
                continue;
            }
            // Move numbers start at 1; some real books contain 0.
            let ply = fields.get(3).and_then(|p| p.parse::<u32>().ok()).map(|p| p.max(1));
            // Normalise through the rules engine (hand order etc.); invalid positions are dropped.
            let Some(board) = Book::board(&fields[..3].join(" ")) else {
                report.invalid_positions += 1;
                current = None;
                continue;
            };
            let key = Book::key(&board);
            if book.positions.len() >= limits.max_positions && !book.positions.contains_key(&key) {
                return Err(BookError::TooLarge { what: "positions", limit: limits.max_positions });
            }
            let pos = book.positions.entry(key.clone()).or_insert_with(BookPosition::default);
            if pos.ply.is_some() || !pos.moves.is_empty() {
                report.duplicate_positions += 1;
            }
            pos.ply = match (pos.ply, ply) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if let Some(c) = comment {
                add_line(&mut pos.comment, c, false);
            }
            sections.push((key.clone(), no, usize::MAX));
            current = Some(key);
            continue;
        }
        let Some(key) = &current else { continue };
        let mut t = line.split_whitespace();
        let Some(mv) = t.next().and_then(|m| Move16::from_usi(m).ok()) else {
            report.illegal_moves += 1;
            continue;
        };
        let ponder = t.next().and_then(|p| Move16::from_usi(p).ok()).map(Move16Ser);
        let value = parse_opt_i32(t.next());
        let depth = parse_opt_i32(t.next()).map(|d| d.max(0) as u32);
        let count = t.next().and_then(|c| c.parse::<u64>().ok()).unwrap_or(1);
        let pos = book.positions.get_mut(key).ok_or_else(|| BookError::Corrupt(format!("line {}", no + 1)))?;
        if pos.moves.len() >= limits.max_moves_per_position {
            return Err(BookError::TooLarge { what: "moves in one position", limit: limits.max_moves_per_position });
        }
        match pos.find_mut(mv) {
            Some(e) => {
                e.count = e.count.saturating_add(count);
                if let Some(c) = comment {
                    add_line(&mut e.comment, c, false);
                }
            }
            None => pos.moves.push(BookMove { mv: Move16Ser(mv), ponder, value, depth, count, wins: None, mark: Default::default(), comment: comment.unwrap_or("").to_owned() }),
        }
    }
    // Positions listed more than once keep their sections' original text (SPG §10.3).
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (k, _, _) in &sections {
        *seen.entry(k.as_str()).or_default() += 1;
    }
    for (k, start, end) in &sections {
        if seen[k.as_str()] > 1
            && let Some(p) = book.positions.get_mut(k)
        {
            let end = (*end).min(lines.len());
            add_line(&mut p.by_ply, &lines[*start..end].join("\n"), true);
        }
    }
    Ok((book, report))
}

pub fn write(book: &Book) -> Vec<u8> {
    let mut lines: Vec<(String, &BookPosition)> = book
        .positions
        .iter()
        .filter(|(_, p)| !p.moves.is_empty())
        .map(|(k, p)| (format!("sfen {k} {}", p.ply.unwrap_or(1).max(1)), p))
        .collect();
    lines.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut out = String::with_capacity(lines.len() * 64);
    out.push_str(HEADER);
    out.push('\n');
    out.push_str(&format!("# NOE:{}\n", lines.len()));
    let comment_lines = |out: &mut String, text: &str| {
        for l in text.lines() {
            out.push_str("// ");
            out.push_str(l);
            out.push('\n');
        }
    };
    comment_lines(&mut out, &book.description);
    for (head, pos) in lines {
        out.push_str(&head);
        out.push('\n');
        comment_lines(&mut out, &pos.comment);
        let mut moves: Vec<&BookMove> = pos.moves.iter().collect();
        moves.sort_by(|a, b| b.count.cmp(&a.count).then(b.value.unwrap_or(i32::MIN).cmp(&a.value.unwrap_or(i32::MIN))));
        for m in moves {
            out.push_str(&format!(
                "{} {} {} {} {}",
                m.mv.0.to_usi(),
                m.ponder.map(|p| p.0.to_usi()).unwrap_or_else(|| "none".into()),
                // "none" keeps "not given" apart from 0 (YaneuraOu reads both).
                m.value.map_or("none".into(), |v| v.to_string()),
                m.depth.map_or("none".into(), |v| v.to_string()),
                m.count.max(1)
            ));
            if !m.comment.is_empty() {
                // One line per move: a multi-line comment is joined with spaces.
                out.push_str(" // ");
                out.push_str(&m.comment.lines().collect::<Vec<_>>().join(" "));
            }
            out.push('\n');
        }
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_sorting() {
        let book = crate::tests::sample();
        let bytes = write(&book);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with(HEADER));
        let sfens: Vec<&str> = text.lines().filter(|l| l.starts_with("sfen ")).collect();
        let mut sorted = sfens.clone();
        sorted.sort();
        assert_eq!(sfens, sorted);
        assert!(text.contains("7g7f 3c3d 40 20 10"));
        let (back, report) = read(&bytes, &BookLimits::default()).unwrap();
        assert_eq!(report.invalid_positions, 0);
        assert_eq!(back.positions.len(), book.positions.len());
        assert_eq!(write(&back), bytes);
    }

    #[test]
    fn lenient_fields_and_bad_input() {
        let text = "#YANEURAOU-DB2016 1.00\n// comment\nsfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1\n7g7f none\n2g2f 8c8d none none // trailing\nzzzz\nsfen garbage b - 1\n7g7f\n";
        let (b, r) = read(text.as_bytes(), &BookLimits::default()).unwrap();
        let p = b.positions.values().next().unwrap();
        assert_eq!(p.moves.len(), 2);
        assert_eq!((p.moves[0].value, p.moves[0].count), (None, 1));
        assert_eq!(p.moves[1].comment, "trailing");
        assert_eq!(b.description, "comment");
        assert!(p.by_ply.is_empty());
        assert_eq!(r.illegal_moves, 1);
        assert_eq!(r.invalid_positions, 1);
        assert!(read(&[0xff, 0xfe], &BookLimits::default()).is_err());
    }

    /// A position listed at two plies is merged, and the original sections are kept.
    #[test]
    fn position_at_several_plies() {
        let s1 = "sfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1\n7g7f none 30 10 2";
        let s2 = "sfen lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 3\n2g2f none 20 10 1 // later";
        let text = format!("#YANEURAOU-DB2016 1.00\n{s1}\n{s2}\n");
        let (b, r) = read(text.as_bytes(), &BookLimits::default()).unwrap();
        assert_eq!(r.duplicate_positions, 1);
        let p = b.positions.values().next().unwrap();
        assert_eq!(p.moves.len(), 2);
        assert_eq!(p.ply, Some(1));
        assert_eq!(p.by_ply, format!("{s1}\n{s2}"));
    }

    /// tanuki's book, when present (`SAKIYOMI_TANUKI_DIR`, else `engines/tanuki-dr4` of the
    /// shogi_GPU_AI project): read, write, re-read.
    #[test]
    fn real_yaneuraou_book() {
        let dir = std::env::var_os("SAKIYOMI_TANUKI_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../engines/tanuki-dr4"));
        let Ok(bytes) = std::fs::read(dir.join("book/user_book1.db")) else { return };
        let (book, report) = crate::read(crate::BookFormat::YaneuraOuDb, &bytes, &crate::ReadOptions::default()).unwrap();
        assert!(book.positions.len() > 1000, "{report}");
        assert_eq!(report.illegal_moves, 0, "{report}");
        let out = write(&book);
        let (mut again, _) = read(&out, &BookLimits::default()).unwrap();
        let mut book = book;
        for b in [&mut again, &mut book] {
            b.positions.retain(|_, p| !p.moves.is_empty());
            b.positions.values_mut().for_each(crate::BookPosition::sort_moves);
        }
        assert_eq!(again.positions.len(), book.positions.len());
        for (k, p) in &book.positions {
            assert_eq!(again.positions.get(k), Some(p), "{k}");
        }
    }
}
