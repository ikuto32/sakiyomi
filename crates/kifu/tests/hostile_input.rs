//! Readers must never panic, whatever they are given: valid records with random damage
//! (flipped characters, cut lines, shuffled and duplicated lines) and plain garbage.
//! Errors are fine; panics, hangs and runaway memory are not.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use sakiyomi_kifu::db::{DbFormat, GameReader};
use sakiyomi_kifu::eval_comment::EvalStyle;
use sakiyomi_kifu::format::{self, Format, TextEncoding};
use sakiyomi_kifu::{GameRecord, Limits, csa, kif};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Characters that matter to the parsers, so damage hits interesting paths.
const ALPHABET: &[&str] = &[
    "▲", "△", "同", "　", "成", "不成", "打", "右", "左", "上", "引", "寄", "直", "*", "**", "&", "#", "+", "-", "%",
    "P", "N", "'", "/", "|", "v", " ", "\n", "0", "9", "１", "九", "歩", "玉", "竜", "変化：", "手数", "：", "(",
    ")", "{", "}", "[", "]", "\"", ",", "position", "sfen", "moves", "startpos", "TORYO", "T", "V2.2", "PI", "P1",
];

fn sample_record() -> GameRecord {
    let mut rec = format::read_usi(
        "position startpos moves 7g7f 3c3d 2g2f 4c4d 2f2e 2b3c 3i4h 3a3b 5i6h 8b4b 6h7h 5a6b 4i5h 7a7b 9g9f 9c9d",
        &Limits::default(),
    )
    .unwrap();
    // A variation and a comment so every writer emits branches and comments.
    // Node 2 is the position after 7g7f 3c3d on the main line.
    let alt = rec.add_usi(2, "6g6f").unwrap();
    rec.add_usi(alt, "8c8d").ok();
    rec.nodes[1].comment = "コメント\n*解析 0 ○ 深さ 10 評価値 100 読み筋 △３四歩(33)".into();
    rec
}

fn damage(rng: &mut Rng, text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for _ in 0..1 + rng.below(4) {
        if lines.is_empty() {
            break;
        }
        let i = rng.below(lines.len());
        match rng.below(6) {
            0 => {
                // Replace a character with something meaningful to the parser.
                let chars: Vec<char> = lines[i].chars().collect();
                let at = rng.below(chars.len() + 1);
                let mut s: String = chars[..at.min(chars.len())].iter().collect();
                s.push_str(ALPHABET[rng.below(ALPHABET.len())]);
                s.extend(chars.iter().skip(at + 1));
                lines[i] = s;
            }
            1 => {
                let chars: Vec<char> = lines[i].chars().collect();
                lines[i] = chars[..rng.below(chars.len() + 1)].iter().collect();
            }
            2 => {
                let j = rng.below(lines.len());
                lines.swap(i, j);
            }
            3 => {
                let l = lines[i].clone();
                lines.insert(rng.below(lines.len() + 1), l);
            }
            4 => {
                lines.remove(i);
            }
            _ => lines.insert(i, ALPHABET[rng.below(ALPHABET.len())].repeat(1 + rng.below(40))),
        }
    }
    lines.join("\n")
}

fn read_all(text: &str) {
    let limits = Limits::default();
    let _ = format::read(text, None, &limits);
    for f in [Format::Kif, Format::Ki2, Format::Csa, Format::Jkf, Format::Usi] {
        if let Ok(rec) = format::read(text, Some(f), &limits) {
            // Whatever was accepted must also be writable in every format.
            for out in [Format::Kif, Format::Ki2, Format::Csa, Format::Jkf, Format::Usi] {
                let _ = format::write(&rec, out, TextEncoding::Utf8, Some(EvalStyle::ShogiGui));
            }
        }
    }
    let _ = kif::read_bod(text, &limits);
    let _ = csa::read_csa_position(text, &limits);
    let _ = format::decode(text.as_bytes());
}

#[test]
fn damaged_records_never_panic() {
    let rec = sample_record();
    let styles = [Some(EvalStyle::ShogiGui), Some(EvalStyle::ShogiHome), Some(EvalStyle::Floodgate), None];
    let mut seeds = Vec::new();
    for f in [Format::Kif, Format::Ki2, Format::Csa, Format::Jkf, Format::Usi] {
        for s in styles {
            seeds.push(format::write(&rec, f, TextEncoding::Utf8, s).unwrap());
        }
    }
    seeds.push(kif::bod(&rec.nodes[3].board, rec.side_names()));
    let seed = std::env::var("SAKIYOMI_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut rng = Rng(seed | 1);
    // SAKIYOMI_FUZZ_ROUNDS raises the count for longer local runs.
    let rounds: usize = std::env::var("SAKIYOMI_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    for round in 0..rounds {
        let seed = &seeds[round % seeds.len()];
        read_all(&damage(&mut rng, seed));
    }
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for len in [0usize, 1, 2, 3, 7, 31, 200, 3000] {
        for _ in 0..20 {
            let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            read_all(&format::decode(&bytes));
            let tokens: String = (0..len / 2).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect();
            read_all(&tokens);
            for f in [DbFormat::Hcpe, DbFormat::Psv, DbFormat::Hcpe3, DbFormat::SfenLines, DbFormat::CsaMulti] {
                for g in GameReader::new(f, Cursor::new(bytes.clone()), Limits::default()).take(100) {
                    let _ = g;
                }
            }
        }
    }
}

#[test]
fn limits_are_enforced() {
    let limits = Limits { max_nodes: 50, ..Limits::default() };
    let long = format!("position startpos moves {}", ["5i5h 5a5b 5h5i 5b5a"; 40].join(" "));
    assert!(format::read_usi(&long, &limits).is_err());
    let huge_line = "a".repeat(Limits::default().max_line_bytes + 10);
    let _ = format::read(&huge_line, None, &Limits::default());
    // Deeply nested JKF forks are refused rather than recursed into.
    let mut jkf = String::from(r#"{"header":{},"moves":[{}"#);
    for _ in 0..5000 {
        jkf.push_str(r#",{"move":{"to":{"x":5,"y":8},"from":{"x":5,"y":9},"piece":"OU","color":0},"forks":[[{}"#);
    }
    let _ = format::read(&jkf, Some(Format::Jkf), &Limits::default());
}
