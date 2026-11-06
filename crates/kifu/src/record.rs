//! Game record as a tree of moves (the first child is the main line).

use serde::Serialize;
use shogi_core::rules::Board;
use shogi_core::{Move16, START_SFEN};

use crate::error::{Error, ErrorKind, Result};
use crate::notation;

/// Evaluation from Sente's (先手, Black's) point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Eval {
    /// Centipawns; mate scores are mapped to ±(MATE_CP − plies) so they sort correctly.
    pub cp: i32,
    /// Plies to mate (positive = Sente mates), when the score is a mate score.
    pub mate: Option<i32>,
}

pub const MATE_CP: i32 = 30000;

impl Eval {
    /// From a score relative to the side to move (as USI engines and most files report it).
    pub fn from_side_to_move(cp: i32, mate: Option<i32>, side_to_move: u8) -> Self {
        let sign = if side_to_move == 0 { 1 } else { -1 };
        match mate {
            None => Eval { cp: cp.clamp(-MATE_CP + 1000, MATE_CP - 1000) * sign, mate: None },
            Some(n) => {
                let n = n.clamp(-999, 999);
                let cp = if n >= 0 { MATE_CP - n } else { -MATE_CP - n };
                Eval { cp: cp * sign, mate: Some(n * sign) }
            }
        }
    }

    pub fn cp(cp: i32) -> Self {
        Eval { cp, mate: None }
    }

    /// The score relative to `side` (inverse of [`Eval::from_side_to_move`]).
    pub fn for_side(&self, side: u8) -> (i32, Option<i32>) {
        let sign = if side == 0 { 1 } else { -1 };
        (self.cp * sign, self.mate.map(|m| m * sign))
    }
}

/// One engine's reading of a position.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineEval {
    pub eval: Option<Eval>,
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    pub nodes: Option<u64>,
    pub time_ms: Option<u64>,
    /// Principal variation starting from the position *before* the node's move
    /// (i.e. the engine's expected line, beginning with the move it chose).
    pub pv: Vec<String>,
    pub engine: Option<String>,
    /// The score's point of view was inferred from a comment convention, not stated by the
    /// source (SPG stores such scores as "viewpoint unknown").
    pub assumed_viewpoint: bool,
}

/// Readings attached to a node: the engine that played Sente, the one that played Gote,
/// and a separate analysis engine. Kept apart so both sides can be drawn in the graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NodeAnalysis {
    pub black: Option<EngineEval>,
    pub white: Option<EngineEval>,
    pub analysis: Option<EngineEval>,
}

impl NodeAnalysis {
    pub fn is_empty(&self) -> bool {
        self.black.is_none() && self.white.is_none() && self.analysis.is_none()
    }

    pub fn slot(&self, side: u8) -> Option<&EngineEval> {
        if side == 0 { self.black.as_ref() } else { self.white.as_ref() }
    }

    pub fn slot_mut(&mut self, side: u8) -> &mut Option<EngineEval> {
        if side == 0 { &mut self.black } else { &mut self.white }
    }
}

/// Terminal markers that end a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Special {
    Resign,
    Interrupt,
    Sennichite,
    Checkmate,
    Jishogi,
    TimeUp,
    /// The side to move wins because the previous move was illegal.
    IllegalWin,
    /// The side to move loses because of its own illegal action.
    IllegalLoss,
    EnteringKingWin,
    WinByDefault,
    LossByDefault,
    /// 不詰: a problem without a mate.
    NoMate,
    /// 引き分け (CSA %HIKIWAKE).
    Draw,
    /// 最大手数 (CSA %MAX_MOVES).
    MaxMoves,
    /// 待った: the preceding moves were taken back (CSA %MATTA).
    Matta,
}

impl Special {
    pub const ALL: [Special; 15] = [
        Special::Resign,
        Special::Interrupt,
        Special::Sennichite,
        Special::Checkmate,
        Special::Jishogi,
        Special::TimeUp,
        Special::IllegalWin,
        Special::IllegalLoss,
        Special::EnteringKingWin,
        Special::WinByDefault,
        Special::LossByDefault,
        Special::NoMate,
        Special::Draw,
        Special::MaxMoves,
        Special::Matta,
    ];

    /// KIF name.
    pub fn kif_name(self) -> &'static str {
        match self {
            Special::Resign => "投了",
            Special::Interrupt => "中断",
            Special::Sennichite => "千日手",
            Special::Checkmate => "詰み",
            Special::Jishogi => "持将棋",
            Special::TimeUp => "切れ負け",
            Special::IllegalWin => "反則勝ち",
            Special::IllegalLoss => "反則負け",
            Special::EnteringKingWin => "入玉勝ち",
            Special::WinByDefault => "不戦勝",
            Special::LossByDefault => "不戦敗",
            Special::NoMate => "不詰",
            Special::Draw => "引き分け",
            Special::MaxMoves => "最大手数",
            Special::Matta => "待った",
        }
    }

    pub fn from_kif(name: &str) -> Option<Special> {
        match name {
            "詰" => Some(Special::Checkmate),
            "時間切れ" => Some(Special::TimeUp),
            _ => Special::ALL.into_iter().find(|s| s.kif_name() == name),
        }
    }

    /// Winner given the side to move at the terminal node; `None` for draws/interruptions.
    pub fn winner(self, side_to_move: u8) -> Option<u8> {
        match self {
            Special::Resign | Special::Checkmate | Special::TimeUp | Special::IllegalLoss | Special::LossByDefault => {
                Some(1 - side_to_move)
            }
            Special::IllegalWin | Special::EnteringKingWin | Special::WinByDefault => Some(side_to_move),
            Special::Interrupt | Special::Sennichite | Special::Jishogi | Special::NoMate | Special::Draw | Special::MaxMoves | Special::Matta => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub mv: Option<Move16>,
    pub special: Option<Special>,
    /// Position after this node.
    pub board: Board,
    pub ply: u32,
    /// KI2-style label with side mark (▲７六歩); empty for the root and specials.
    pub label: String,
    /// Western notation (☗P-7f); empty for the root and specials.
    pub western: String,
    pub comment: String,
    pub time_ms: Option<u64>,
    pub analysis: NodeAnalysis,
}

#[derive(Clone, Debug)]
pub struct GameRecord {
    pub headers: Vec<(String, String)>,
    pub nodes: Vec<Node>,
}

pub const HANDICAPS: &[(&str, &str)] = &[
    ("平手", START_SFEN),
    ("香落ち", "lnsgkgsn1/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("右香落ち", "1nsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("角落ち", "lnsgkgsnl/1r7/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("飛車落ち", "lnsgkgsnl/7b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("飛香落ち", "lnsgkgsn1/7b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("二枚落ち", "lnsgkgsnl/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("三枚落ち", "lnsgkgsn1/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("四枚落ち", "1nsgkgsn1/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("五枚落ち", "2sgkgsn1/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("左五枚落ち", "1nsgkgs2/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("六枚落ち", "2sgkgs2/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("左七枚落ち", "2sgkg3/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("右七枚落ち", "3gkgs2/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("八枚落ち", "3gkg3/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
    ("十枚落ち", "4k4/9/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL w - 1"),
];

pub fn handicap_sfen(name: &str) -> Option<&'static str> {
    let name = name.trim();
    HANDICAPS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

pub fn handicap_name(board: &Board) -> Option<&'static str> {
    HANDICAPS
        .iter()
        .find(|(_, s)| Board::from_sfen(s).map(|b| b.key() == board.key()).unwrap_or(false))
        .map(|(n, _)| *n)
}

/// Parse a SFEN into a board, as a typed error.
pub fn parse_sfen(sfen: &str) -> Result<Board> {
    Board::from_sfen(sfen.trim()).map_err(|e| ErrorKind::InvalidSfen(e.to_string()).into())
}

/// Build a board from raw parts (shogi-core piece codes), validating it through SFEN.
pub fn board_from_parts(squares: [u8; 81], hands: [[u8; 7]; 2], side: u8) -> Result<Board> {
    let b = Board { squares, hands, side, move_number: 1 };
    Board::from_sfen(&b.to_sfen()).map_err(|e| ErrorKind::InvalidPosition(e.to_string()).into())
}

/// SFEN without the trailing move number: the key used for books and transpositions.
pub fn sfen_key(board: &Board) -> String {
    let s = board.to_sfen();
    match s.rfind(' ') {
        Some(i) => s[..i].to_owned(),
        None => s,
    }
}

/// The initial position.
#[allow(clippy::expect_used)] // START_SFEN is a constant that always parses.
pub fn start_board() -> Board {
    Board::from_sfen(START_SFEN).expect("START_SFEN parses")
}

impl GameRecord {
    /// A record starting from the initial position.
    #[allow(clippy::expect_used)] // START_SFEN is a constant that always parses.
    pub fn from_start() -> Self {
        GameRecord::from_sfen(START_SFEN).expect("START_SFEN parses")
    }

    pub fn new(board: Board) -> Self {
        GameRecord {
            headers: Vec::new(),
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                mv: None,
                special: None,
                board,
                ply: 0,
                label: String::new(),
                western: String::new(),
                comment: String::new(),
                time_ms: None,
                analysis: NodeAnalysis::default(),
            }],
        }
    }

    pub fn from_sfen(sfen: &str) -> Result<Self> {
        Ok(Self::new(parse_sfen(sfen)?))
    }

    pub fn initial(&self) -> &Board {
        &self.nodes[0].board
    }

    /// The SFEN of the start position (move number normalised to 1).
    pub fn initial_sfen(&self) -> String {
        let mut b = *self.initial();
        b.move_number = 1;
        b.to_sfen()
    }

    pub fn is_handicap(&self) -> bool {
        matches!(handicap_name(self.initial()), Some(n) if n != "平手")
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn set_header(&mut self, key: &str, value: &str) {
        if let Some(h) = self.headers.iter_mut().find(|(k, _)| k == key) {
            h.1 = value.to_owned();
        } else {
            self.headers.push((key.to_owned(), value.to_owned()));
        }
    }

    pub fn black_name(&self) -> Option<&str> {
        self.header("先手").or_else(|| self.header("下手"))
    }

    pub fn white_name(&self) -> Option<&str> {
        self.header("後手").or_else(|| self.header("上手"))
    }

    pub fn prev_to(&self, node: usize) -> Option<usize> {
        self.nodes[node].mv.map(|m| m.destination().index())
    }

    pub fn add_move(&mut self, parent: usize, mv: Move16) -> Result<usize> {
        if let Some(&c) = self.nodes[parent].children.iter().find(|&&c| self.nodes[c].mv == Some(mv)) {
            return Ok(c);
        }
        let p = &self.nodes[parent];
        if p.special.is_some() {
            return Err(ErrorKind::GameAlreadyOver.into());
        }
        let legal = p.board.legal_moves();
        if !legal.contains(&mv) {
            return Err(ErrorKind::IllegalMove(mv.to_usi()).into());
        }
        let label = notation::ki2_move_with(&p.board, mv, self.prev_to(parent), &legal);
        let western = notation::western_move_with(&p.board, mv, &legal);
        let node = Node {
            parent: Some(parent),
            children: Vec::new(),
            mv: Some(mv),
            special: None,
            board: p.board.after_generated(mv),
            ply: p.ply + 1,
            label,
            western,
            comment: String::new(),
            time_ms: None,
            analysis: NodeAnalysis::default(),
        };
        Ok(self.push(parent, node))
    }

    /// Add a USI move (e.g. "7g7f") as a child of `parent`.
    pub fn add_usi(&mut self, parent: usize, usi: &str) -> Result<usize> {
        let mv = Move16::from_usi(usi).map_err(|_| Error::new(ErrorKind::UnparsableMove(usi.to_owned())))?;
        self.add_move(parent, mv)
    }

    pub fn add_special(&mut self, parent: usize, special: Special) -> usize {
        if let Some(&c) = self.nodes[parent].children.iter().find(|&&c| self.nodes[c].special == Some(special)) {
            return c;
        }
        let p = &self.nodes[parent];
        let node = Node {
            parent: Some(parent),
            children: Vec::new(),
            mv: None,
            special: Some(special),
            board: p.board,
            ply: p.ply + 1,
            label: String::new(),
            western: String::new(),
            comment: String::new(),
            time_ms: None,
            analysis: NodeAnalysis::default(),
        };
        self.push(parent, node)
    }

    fn push(&mut self, parent: usize, node: Node) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        self.nodes[parent].children.push(id);
        id
    }

    pub fn path(&self, node: usize) -> Vec<usize> {
        let mut v = vec![node];
        let mut n = node;
        while let Some(p) = self.nodes[n].parent {
            v.push(p);
            n = p;
        }
        v.reverse();
        v
    }

    pub fn moves_to(&self, node: usize) -> Vec<Move16> {
        self.path(node).iter().filter_map(|&n| self.nodes[n].mv).collect()
    }

    pub fn line_end(&self, node: usize) -> usize {
        let mut n = node;
        while let Some(&c) = self.nodes[n].children.first() {
            n = c;
        }
        n
    }

    pub fn mainline(&self) -> Vec<usize> {
        self.path(self.line_end(0))
    }

    /// Cumulative time used by the side that played `node`, along its path.
    pub fn cumulative_ms(&self, node: usize) -> u64 {
        let parity = self.nodes[node].ply % 2;
        self.path(node)
            .iter()
            .filter(|&&n| n != 0 && self.nodes[n].ply % 2 == parity)
            .filter_map(|&n| self.nodes[n].time_ms)
            .fold(0u64, |a, b| a.saturating_add(b))
    }

    /// Make `node` the first (main-line) child of its parent.
    pub fn promote_variation(&mut self, node: usize) {
        if let Some(p) = self.nodes[node].parent {
            let ch = &mut self.nodes[p].children;
            if let Some(i) = ch.iter().position(|&c| c == node) {
                let n = ch.remove(i);
                ch.insert(0, n);
            }
        }
    }

    /// Remove `node` and its subtree; returns the (re-indexed) parent and an index map.
    pub fn remove_subtree(&mut self, node: usize) -> Option<(usize, Vec<Option<usize>>)> {
        let parent = self.nodes[node].parent?;
        let mut dead = vec![false; self.nodes.len()];
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            dead[n] = true;
            stack.extend(self.nodes[n].children.iter().copied());
        }
        let mut map = vec![None; self.nodes.len()];
        let mut next = 0;
        for (i, d) in dead.iter().enumerate() {
            if !d {
                map[i] = Some(next);
                next += 1;
            }
        }
        let old = std::mem::take(&mut self.nodes);
        for (i, mut n) in old.into_iter().enumerate() {
            if dead[i] {
                continue;
            }
            n.parent = n.parent.and_then(|p| map[p]);
            n.children = n.children.iter().filter_map(|&c| map[c]).collect();
            self.nodes.push(n);
        }
        Some((map[parent]?, map))
    }

    /// Black/White labels: 先手/後手, or 下手/上手 for handicap games.
    pub fn side_names(&self) -> (&'static str, &'static str) {
        if self.is_handicap() { ("下手", "上手") } else { ("先手", "後手") }
    }

    /// Result sentence for KIF/KI2 (まで…), when the main line ends with a special.
    pub fn result_sentence(&self) -> Option<String> {
        let end = self.line_end(0);
        let n = &self.nodes[end];
        let special = n.special?;
        let moves = n.ply.saturating_sub(1);
        let (b, w) = self.side_names();
        let to_move = n.board.side;
        let (mover_name, other_name) = if to_move == 0 { (b, w) } else { (w, b) };
        Some(match special {
            Special::Resign | Special::Checkmate => format!("まで{moves}手で{other_name}の勝ち"),
            Special::TimeUp => format!("まで{moves}手で時間切れにより{other_name}の勝ち"),
            Special::IllegalLoss => format!("まで{moves}手で{mover_name}の反則負け"),
            Special::IllegalWin => format!("まで{moves}手で{other_name}の反則負け"),
            Special::EnteringKingWin => format!("まで{moves}手で{mover_name}の入玉勝ち"),
            Special::Sennichite => format!("まで{moves}手で千日手"),
            Special::Jishogi => format!("まで{moves}手で持将棋"),
            Special::Draw => format!("まで{moves}手で引き分け"),
            other => format!("まで{moves}手で{}", other.kif_name()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_operations() {
        let mut r = GameRecord::from_sfen(START_SFEN).unwrap();
        let a = r.add_usi(0, "7g7f").unwrap();
        let b = r.add_usi(a, "3c3d").unwrap();
        let alt = r.add_usi(0, "2g2f").unwrap();
        assert_eq!(r.add_usi(0, "7g7f").unwrap(), a);
        assert!(r.add_usi(b, "7f7e").is_ok());
        assert_eq!(r.add_usi(b, "5a5c").unwrap_err().kind, ErrorKind::IllegalMove("5a5c".into()));
        assert_eq!(r.mainline().len(), 4);
        r.promote_variation(alt);
        assert_eq!(r.nodes[0].children[0], alt);
        let (p, _) = r.remove_subtree(a).unwrap();
        assert_eq!(p, 0);
        assert_eq!(r.nodes.len(), 2);
        assert_eq!(r.nodes[1].label, "▲２六歩");
        assert_eq!(r.nodes[1].western, "☗P-2f");
        assert_eq!(handicap_name(&Board::from_sfen(HANDICAPS[3].1).unwrap()), Some("角落ち"));
        assert_eq!(sfen_key(r.initial()), "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b -");
    }

    #[test]
    fn eval_perspective() {
        let e = Eval::from_side_to_move(120, None, 1);
        assert_eq!(e, Eval::cp(-120));
        assert_eq!(e.for_side(1), (120, None));
        let m = Eval::from_side_to_move(0, Some(3), 1);
        assert_eq!(m.mate, Some(-3));
        assert!(m.cp < -29000);
        assert_eq!(Special::Resign.winner(1), Some(0));
        assert_eq!(Special::from_kif("詰"), Some(Special::Checkmate));
    }
}
