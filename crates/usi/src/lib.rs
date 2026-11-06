//! USI engine client: process management, handshake, option and info parsing, and a
//! persistent registry of engines with per-engine option values.
//!
//! Engines run only from a path the user registered, without a shell or arguments, in
//! their own directory; output lines are length-limited and processes are killed on drop.

pub mod protocol;
pub mod registry;

pub use protocol::{EngineId, GoParams, Info, OptionDef, Score, UsiError, UsiProcess, parse_info, parse_option, start_engine};
pub use registry::{EngineConfig, Registry};
