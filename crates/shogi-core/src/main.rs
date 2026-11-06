use shogi_core::rules::Board;
use shogi_core::{Move16, Position, Square};
use std::io::{self, BufRead};

fn main() {
    // A dependency-free JSONL bridge for oracle parity tests. Input: SFEN per line,
    // or "move <usi>" for Move16 encoding. Invalid input is an error and exits 1.
    for line in io::stdin().lock().lines() {
        let line = match line {
            Ok(value) => value,
            Err(error) => fail(&error.to_string()),
        };
        if let Some(text) = line.strip_prefix("move ") {
            let value = Move16::from_usi(text).unwrap_or_else(|error| fail(&error.to_string()));
            println!(
                "{{\"packed\":{},\"usi\":\"{}\"}}",
                value.packed(),
                value.to_usi()
            );
            continue;
        }
        if let Some(text) = line.strip_prefix("legal ") {
            let board = Board::from_sfen(text).unwrap_or_else(|error| fail(&error.to_string()));
            let mut moves: Vec<_> = board.legal_moves().iter().map(|mv| mv.to_usi()).collect();
            moves.sort();
            println!(
                "{{\"moves\":{:?},\"check\":{},\"declaration\":{}}}",
                moves,
                board.in_check(board.side),
                board.declaration_win()
            );
            continue;
        }
        if let Some(text) = line.strip_prefix("mate ") {
            let mut fields = text.splitn(3, ' ');
            let depth = fields
                .next()
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|n| *n > 0 && *n <= 31 && n % 2 == 1)
                .unwrap_or_else(|| fail("Invalid mate depth"));
            let nodes = fields
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n > 0)
                .unwrap_or_else(|| fail("Invalid mate node limit"));
            let board = Board::from_sfen(fields.next().unwrap_or(""))
                .unwrap_or_else(|e| fail(&e.to_string()));
            let stop = std::sync::atomic::AtomicBool::new(false);
            let result = shogi_core::mate::prove_mate(&board, depth, nodes, None, &stop);
            let pv: Vec<_> = result.pv.iter().map(|m| m.to_usi()).collect();
            println!(
                "{{\"status\":\"{:?}\",\"nodes\":{},\"pv\":{:?}}}",
                result.status, result.nodes, pv
            );
            continue;
        }
        if let Some(text) = line.strip_prefix("perft ") {
            let (depth, sfen) = text
                .split_once(' ')
                .unwrap_or_else(|| fail("perft <depth> <sfen>"));
            let depth = depth
                .parse::<u8>()
                .unwrap_or_else(|_| fail("Invalid depth"));
            if depth > 6 {
                fail("Perft depth exceeds six");
            }
            let board = Board::from_sfen(sfen).unwrap_or_else(|error| fail(&error.to_string()));
            let mut divide: Vec<_> = if depth == 0 {
                vec![]
            } else {
                board
                    .legal_moves()
                    .into_iter()
                    .map(|mv| (mv.to_usi(), board.after_generated(mv).perft(depth - 1)))
                    .collect()
            };
            divide.sort();
            let nodes = if depth == 0 {
                1
            } else {
                divide.iter().map(|(_, n)| n).sum()
            };
            let fields: Vec<_> = divide
                .iter()
                .map(|(mv, n)| format!("\"{mv}\":{n}"))
                .collect();
            println!(
                "{{\"nodes\":{},\"divide\":{{{}}}}}",
                nodes,
                fields.join(",")
            );
            continue;
        }
        if let Some(text) = line.strip_prefix("play ") {
            let (mv, sfen) = text
                .split_once(' ')
                .unwrap_or_else(|| fail("play <usi> <sfen>"));
            let board = Board::from_sfen(sfen).unwrap_or_else(|error| fail(&error.to_string()));
            let mv = Move16::from_usi(mv).unwrap_or_else(|error| fail(&error.to_string()));
            let child = board
                .play(mv)
                .unwrap_or_else(|error| fail(&error.to_string()));
            println!("{{\"sfen\":\"{}\"}}", child.to_sfen());
            continue;
        }
        let position = Position::from_sfen(&line).unwrap_or_else(|error| fail(&error.to_string()));
        let pieces: Vec<_> = (0..81)
            .map(|i| position.piece_at(Square::from_index(i).unwrap()))
            .collect();
        println!(
            "{{\"sfen\":\"{}\",\"pieces\":{:?},\"hands\":{:?},\"side_to_move\":{},\"move_number\":{},\"occupied\":{:?}}}",
            position.to_sfen(),
            pieces,
            position.hands,
            position.side_to_move,
            position.move_number,
            position.occupied.map(|b| b.words())
        );
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
