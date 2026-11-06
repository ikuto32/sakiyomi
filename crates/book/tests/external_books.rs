//! Books written by other programs. Fixtures are not shipped with Sakiyomi; point
//! `SAKIYOMI_BOOK_FIXTURES` at a directory containing them (for example ShogiHome's
//! `src/tests/testdata/book`, which includes .sbk files written by ShogiGUI).

use sakiyomi_book::{BookFormat, ReadOptions, read, write};

#[test]
fn books_from_other_programs() {
    let Ok(dir) = std::env::var("SAKIYOMI_BOOK_FIXTURES") else { return };
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("fixture dir") {
        let path = entry.unwrap().path();
        let Some(fmt) = path.extension().and_then(|e| e.to_str()).and_then(BookFormat::from_name) else { continue };
        if fmt == BookFormat::Gikou && path.extension().unwrap() == "bin" && std::fs::metadata(&path).unwrap().len() < 20992 {
            continue; // Apery .bin, not Gikou
        }
        let bytes = std::fs::read(&path).unwrap();
        let (book, report) = match read(fmt, &bytes, &ReadOptions::default()) {
            Ok(r) => r,
            Err(e) => panic!("{}: {e}", path.display()),
        };
        println!("{}: {report}", path.display());
        // Third-party files may contain illegal moves (they are dropped and reported);
        // our own output must not.
        let _ = report.illegal_moves;
        assert!(!book.positions.is_empty(), "{}", path.display());
        // Every format we read must convert to every format we write and back without loss of moves.
        for target in BookFormat::ALL {
            let (out, _) = write(target, &book, &Default::default()).unwrap();
            // Gikou books store only hash keys: positions are found by walking from roots,
            // so give the reader the source book's positions (as a user would give game records).
            let opts = ReadOptions { roots: book.positions.keys().map(|k| format!("{k} 1")).collect(), ..Default::default() };
            let (back, r2) = read(target, &out, &opts).unwrap();
            assert_eq!(r2.illegal_moves, 0);
            for (k, p) in &book.positions {
                if p.moves.is_empty() {
                    continue;
                }
                let q = back.positions.get(k).unwrap_or_else(|| panic!("{target:?} lost {k}"));
                let mut a: Vec<_> = p.moves.iter().map(|m| m.mv.0.to_usi()).collect();
                let mut b: Vec<_> = q.moves.iter().map(|m| m.mv.0.to_usi()).collect();
                a.sort();
                b.sort();
                assert_eq!(a, b, "{target:?} {k}");
            }
        }
        checked += 1;
    }
    assert!(checked > 0, "no fixtures found in {dir}");
}
