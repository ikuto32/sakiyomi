//! Format detection, text encodings, and SFEN / USI `position` strings.

use shogi_core::{Move16, START_SFEN};

use crate::error::{Error, ErrorKind, Limits, Result, check_text};
use crate::eval_comment::EvalStyle;
use crate::kif::WriteOptions;
use crate::record::GameRecord;
use crate::{csa, jkf, kif};

/// Single-game record formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Kif,
    Ki2,
    Csa,
    Jkf,
    /// `position startpos moves …` / SFEN with moves.
    Usi,
}

impl Format {
    pub const ALL: [Format; 5] = [Format::Kif, Format::Ki2, Format::Csa, Format::Jkf, Format::Usi];

    /// From a file extension or a format name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().trim_start_matches('.') {
            "kif" | "kifu" => Some(Format::Kif),
            "ki2" | "ki2u" => Some(Format::Ki2),
            "csa" => Some(Format::Csa),
            "jkf" | "json" => Some(Format::Jkf),
            "usi" | "sfen" | "txt" => Some(Format::Usi),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Kif => "kif",
            Format::Ki2 => "ki2",
            Format::Csa => "csa",
            Format::Jkf => "jkf",
            Format::Usi => "usi",
        }
    }
}

/// Text encoding for written files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    /// UTF-8 with a byte-order mark (helps other GUIs detect UTF-8 `.kif` files).
    Utf8Bom,
    ShiftJis,
}

/// Decode kifu bytes: UTF-8 (with or without BOM) or Shift_JIS (CP932).
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

/// Encode text with CRLF line endings, as Windows shogi software expects.
pub fn encode(text: &str, encoding: TextEncoding) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    match encoding {
        TextEncoding::Utf8 => text.into_bytes(),
        TextEncoding::Utf8Bom => {
            let mut b = vec![0xEF, 0xBB, 0xBF];
            b.extend_from_slice(text.as_bytes());
            b
        }
        TextEncoding::ShiftJis => encoding_rs::SHIFT_JIS.encode(&text).0.into_owned(),
    }
}

pub fn detect(text: &str) -> Format {
    let t = text.trim_start_matches('\u{feff}').trim();
    if t.starts_with('{') {
        return Format::Jkf;
    }
    let first = t.lines().next().unwrap_or("").trim();
    if first.starts_with("position") || first.starts_with("sfen") || first.starts_with("startpos") || looks_like_sfen(first) {
        return Format::Usi;
    }
    let mut kif = false;
    let mut ki2 = false;
    for line in t.lines().take(10_000) {
        let l = line.trim();
        if l.starts_with("V2") || l == "PI" || l.starts_with("P1") || l.starts_with("N+") || is_csa_move(l) {
            return Format::Csa;
        }
        if l.starts_with("手数----") {
            kif = true;
        }
        let digits = l.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && l[digits..].starts_with(char::is_whitespace) {
            kif = true;
        }
        if l.starts_with(['▲', '△', '☗', '☖']) {
            ki2 = true;
        }
    }
    if ki2 && !kif { Format::Ki2 } else { Format::Kif }
}

fn is_csa_move(l: &str) -> bool {
    let b = l.as_bytes();
    b.len() >= 7
        && (b[0] == b'+' || b[0] == b'-')
        && b[1..5].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_uppercase)
}

fn looks_like_sfen(s: &str) -> bool {
    let f: Vec<&str> = s.split_whitespace().collect();
    f.len() >= 3 && f[0].matches('/').count() == 8 && matches!(f[1], "b" | "w")
}

/// Read one game record; `format = None` detects it.
pub fn read(text: &str, format: Option<Format>, limits: &Limits) -> Result<GameRecord> {
    let text = text.trim_start_matches('\u{feff}');
    match format.unwrap_or_else(|| detect(text)) {
        Format::Kif => kif::read_kif(text, limits),
        Format::Ki2 => kif::read_ki2(text, limits),
        Format::Csa => csa::read_csa(text, limits),
        Format::Jkf => jkf::read_jkf(text, limits),
        Format::Usi => read_usi(text, limits),
    }
}

/// Write one game record.
pub fn write(rec: &GameRecord, format: Format, encoding: TextEncoding, eval_style: Option<EvalStyle>) -> Result<String> {
    let label = if encoding == TextEncoding::ShiftJis { "Shift_JIS" } else { "UTF-8" };
    let opts = WriteOptions { encoding_label: label, eval_style };
    Ok(match format {
        Format::Kif => kif::write_kif(rec, &opts),
        Format::Ki2 => kif::write_ki2(rec, &opts),
        Format::Csa => csa::write_csa(rec, eval_style),
        Format::Jkf => jkf::write_jkf(rec, eval_style)?,
        Format::Usi => usi_position(rec, rec.line_end(0)),
    })
}

/// Accepts `position startpos moves ...`, `position sfen <sfen> moves ...`,
/// `sfen <sfen> ...`, `startpos ...`, or a bare SFEN optionally followed by `moves`.
pub fn read_usi(text: &str, limits: &Limits) -> Result<GameRecord> {
    check_text(text, limits)?;
    let line = text.trim().lines().next().unwrap_or("");
    let (rec, moves) = parse_position_prefix(line)?;
    let mut rec = rec;
    let mut cur = 0;
    for m in moves {
        if rec.nodes.len() >= limits.max_nodes {
            return Err(ErrorKind::TooLarge { what: "moves", limit: limits.max_nodes }.into());
        }
        cur = rec.add_usi(cur, m)?;
    }
    Ok(rec)
}

/// Split a position string into its start position and its move tokens.
pub fn parse_position_prefix(line: &str) -> Result<(GameRecord, Vec<&str>)> {
    let mut tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.first() == Some(&"position") {
        tokens.remove(0);
    }
    let (sfen, rest): (String, &[&str]) = match tokens.first() {
        Some(&"startpos") => (START_SFEN.to_owned(), &tokens[1..]),
        Some(&"sfen") => {
            if tokens.len() < 4 {
                return Err(ErrorKind::InvalidSfen(line.to_owned()).into());
            }
            let n = if tokens.get(4).is_some_and(|t| t.parse::<u32>().is_ok()) { 5 } else { 4 };
            let mut s = tokens[1..n].join(" ");
            if n == 4 {
                s.push_str(" 1");
            }
            (s, &tokens[n..])
        }
        _ if tokens.len() >= 3 => {
            let n = if tokens.get(3).is_some_and(|t| t.parse::<u32>().is_ok()) { 4 } else { 3 };
            let mut s = tokens[..n].join(" ");
            if n == 3 {
                s.push_str(" 1");
            }
            (s, &tokens[n..])
        }
        _ => return Err(Error::new(ErrorKind::InvalidSfen(line.to_owned()))),
    };
    let rec = GameRecord::from_sfen(&sfen)?;
    let moves = if rest.first() == Some(&"moves") { rest[1..].to_vec() } else { Vec::new() };
    Ok((rec, moves))
}

pub fn sfen_at(rec: &GameRecord, node: usize) -> String {
    rec.nodes[node].board.to_sfen()
}

/// `position startpos moves ...` (or `position sfen ...`) leading to `node`.
pub fn usi_position(rec: &GameRecord, node: usize) -> String {
    usi_position_from(&rec.initial_sfen(), &rec.moves_to(node))
}

pub fn usi_position_from(initial_sfen: &str, moves: &[Move16]) -> String {
    let mut s = if initial_sfen == START_SFEN { "position startpos".to_owned() } else { format!("position sfen {initial_sfen}") };
    if !moves.is_empty() {
        s.push_str(" moves");
        for m in moves {
            s.push(' ');
            s.push_str(&m.to_usi());
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usi_position_roundtrip() {
        let l = Limits::default();
        let rec = read("position startpos moves 7g7f 3c3d 8h2b+", None, &l).unwrap();
        assert_eq!(rec.mainline().len(), 4);
        assert_eq!(usi_position(&rec, 3), "position startpos moves 7g7f 3c3d 8h2b+");
        assert_eq!(sfen_at(&rec, 1), "lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 2");
        let r2 = read("lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 2 moves 3c3d", None, &l).unwrap();
        assert!(usi_position(&r2, 1).starts_with("position sfen lnsgkgsnl/1r5b1/ppppppppp/9/9/2P6/PP1PPPPPP/1B5R1/LNSGKGSNL w - 1 moves 3c3d"));
        assert!(read("position startpos moves 7g7f 7g7f", None, &l).is_err());
        assert!(read("sfen 4k4/9/9/9/9/9/9/9/4K4 b -", None, &l).is_ok());
    }

    #[test]
    fn detection_and_shift_jis() {
        assert_eq!(detect("V2.2\nPI\n+\n+7776FU"), Format::Csa);
        assert_eq!(detect("手合割：平手\n手数----指手--\n   1 ７六歩(77)"), Format::Kif);
        assert_eq!(detect("先手：A\n\n▲７六歩    △３四歩"), Format::Ki2);
        assert_eq!(detect("sfen 4k4/9/9/9/9/9/9/9/4K4 b - 1"), Format::Usi);
        assert_eq!(detect("{\"header\":{}}"), Format::Jkf);
        let text = "手合割：平手\n手数----指手---------消費時間--\n   1 ７六歩(77)   ( 0:01/00:00:01)\n   2 投了\n";
        let sjis = encode(text, TextEncoding::ShiftJis);
        assert!(std::str::from_utf8(&sjis).is_err());
        assert_eq!(decode(&sjis), text.replace('\n', "\r\n"));
        let rec = read(&decode(&sjis), None, &Limits::default()).unwrap();
        assert_eq!(rec.mainline().len(), 3);
        assert_eq!(decode(&encode(text, TextEncoding::Utf8Bom)), text.replace('\n', "\r\n"));
    }
}
