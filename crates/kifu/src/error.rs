//! Typed errors for every reader in this crate. `Display` is English; applications
//! localise by matching on [`ErrorKind`].

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A move that is not legal in its position.
    IllegalMove(String),
    /// A Japanese move that matches more than one legal move.
    AmbiguousMove(String),
    /// Text that does not look like a move at all.
    UnparsableMove(String),
    /// 「同」 with no previous move to refer to.
    SameWithoutPrevious(String),
    /// A position that violates the rules (e.g. missing king, too many pieces).
    InvalidPosition(String),
    InvalidSfen(String),
    UnsupportedHandicap(String),
    InvalidHand(String),
    InvalidBoardDiagram(String),
    InvalidVariation(u32),
    InvalidPieceCode(String),
    InvalidSquare(String),
    /// A move was given after the game had already ended.
    GameAlreadyOver,
    /// Input exceeds a configured limit (see [`crate::Limits`]).
    TooLarge { what: &'static str, limit: usize },
    /// Binary input whose length or structure is wrong.
    Corrupt(String),
    /// Valid input that this implementation does not support.
    Unsupported(String),
    /// The format could not be determined.
    UnknownFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    /// 1-based line (text formats) or record index (binary formats).
    pub line: Option<usize>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(kind: ErrorKind) -> Self {
        Error { kind, line: None }
    }

    pub fn at(mut self, line: usize) -> Self {
        if self.line.is_none() {
            self.line = Some(line);
        }
        self
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Error::new(kind)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ErrorKind::*;
        match self {
            IllegalMove(m) => write!(f, "illegal move: {m}"),
            AmbiguousMove(m) => write!(f, "ambiguous move: {m}"),
            UnparsableMove(m) => write!(f, "cannot parse move: {m}"),
            SameWithoutPrevious(m) => write!(f, "\"同\" without a previous move: {m}"),
            InvalidPosition(m) => write!(f, "invalid position: {m}"),
            InvalidSfen(m) => write!(f, "invalid SFEN: {m}"),
            UnsupportedHandicap(m) => write!(f, "unsupported handicap: {m}"),
            InvalidHand(m) => write!(f, "invalid pieces in hand: {m}"),
            InvalidBoardDiagram(m) => write!(f, "invalid board diagram: {m}"),
            InvalidVariation(n) => write!(f, "invalid variation move number: {n}"),
            InvalidPieceCode(m) => write!(f, "invalid piece code: {m}"),
            InvalidSquare(m) => write!(f, "invalid square: {m}"),
            GameAlreadyOver => write!(f, "the game is already over"),
            TooLarge { what, limit } => write!(f, "{what} exceeds the limit of {limit}"),
            Corrupt(m) => write!(f, "corrupt data: {m}"),
            Unsupported(m) => write!(f, "unsupported: {m}"),
            UnknownFormat => write!(f, "unknown format"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.kind),
            None => self.kind.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

/// Resource limits applied by every reader. Defaults suit real-world files with a wide margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum input size in bytes for one text kifu.
    pub max_text_bytes: usize,
    /// Maximum length of one line.
    pub max_line_bytes: usize,
    /// Maximum number of nodes (moves incl. variations) in one record.
    pub max_nodes: usize,
    /// Maximum number of games read from one database file (0 = unlimited).
    pub max_games: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_text_bytes: 64 << 20, max_line_bytes: 1 << 20, max_nodes: 2_000_000, max_games: 0 }
    }
}

pub(crate) fn check_text(text: &str, limits: &Limits) -> Result<()> {
    if text.len() > limits.max_text_bytes {
        return Err(ErrorKind::TooLarge { what: "input", limit: limits.max_text_bytes }.into());
    }
    if let Some((i, _)) = text.lines().enumerate().find(|(_, l)| l.len() > limits.max_line_bytes) {
        return Err(Error::new(ErrorKind::TooLarge { what: "line", limit: limits.max_line_bytes }).at(i + 1));
    }
    Ok(())
}
