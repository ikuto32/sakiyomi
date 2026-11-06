//! Shogi game records and positions, and every text/binary format Sakiyomi reads or writes.
//!
//! Rules (legal moves, SFEN, USI moves) come from `shogi-core`. Every reader takes
//! [`Limits`], returns a typed [`Error`] with a line/record number, and never panics on input.

pub mod csa;
pub mod db;
pub mod error;
pub mod eval_comment;
pub mod format;
pub mod jkf;
pub mod kif;
pub mod notation;
pub mod packed;
pub mod record;

pub use error::{Error, ErrorKind, Limits, Result};
pub use format::{Format, TextEncoding};
pub use record::{Eval, EngineEval, GameRecord, Node, NodeAnalysis, Special};
pub use shogi_core;
