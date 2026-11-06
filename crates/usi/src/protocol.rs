//! USI engine client: process management, handshake, option and info parsing.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Longest engine output line kept (longer lines are truncated).
pub const MAX_LINE: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionDef {
    pub name: String,
    /// spin, check, combo, string, button, filename
    pub kind: String,
    pub default: String,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub vars: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EngineId {
    pub name: String,
    pub author: String,
    pub options: Vec<OptionDef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Score {
    Cp(i32),
    /// Plies to mate; positive = side to move mates. Unknown length is ±999.
    Mate(i32),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Info {
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    pub time_ms: Option<u64>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub hashfull: Option<u32>,
    pub multipv: Option<u32>,
    pub score: Option<Score>,
    /// "lowerbound" / "upperbound"
    pub bound: Option<String>,
    pub pv: Vec<String>,
    pub string: Option<String>,
}

pub fn parse_option(line: &str) -> Option<OptionDef> {
    let rest = line.strip_prefix("option")?.trim_start();
    let rest = rest.strip_prefix("name")?;
    let type_at = rest.find(" type ")?;
    let name = rest[..type_at].trim().to_owned();
    let mut tokens = rest[type_at + 6..].split_whitespace().peekable();
    let kind = tokens.next()?.to_owned();
    let mut def = OptionDef { name, kind, default: String::new(), min: None, max: None, vars: Vec::new() };
    while let Some(key) = tokens.next() {
        match key {
            "default" => {
                let mut parts = Vec::new();
                while let Some(&t) = tokens.peek() {
                    if matches!(t, "min" | "max" | "var") {
                        break;
                    }
                    parts.push(t);
                    tokens.next();
                }
                let v = parts.join(" ");
                def.default = if v == "<empty>" { String::new() } else { v };
            }
            "min" => def.min = tokens.next().and_then(|t| t.parse().ok()),
            "max" => def.max = tokens.next().and_then(|t| t.parse().ok()),
            "var" => {
                if let Some(v) = tokens.next() {
                    def.vars.push(v.to_owned());
                }
            }
            _ => {}
        }
    }
    Some(def)
}

pub fn parse_info(line: &str) -> Option<Info> {
    let mut t = line.split_whitespace();
    if t.next()? != "info" {
        return None;
    }
    let mut info = Info::default();
    let num = |x: Option<&str>| x.and_then(|v| v.parse::<u64>().ok());
    while let Some(key) = t.next() {
        match key {
            "depth" => info.depth = num(t.next()).map(|v| v as u32),
            "seldepth" => info.seldepth = num(t.next()).map(|v| v as u32),
            "time" => info.time_ms = num(t.next()),
            "nodes" => info.nodes = num(t.next()),
            "nps" => info.nps = num(t.next()),
            "hashfull" => info.hashfull = num(t.next()).map(|v| v as u32),
            "multipv" => info.multipv = num(t.next()).map(|v| v as u32),
            "currmove" | "currmovenumber" | "cpuload" | "refutation" | "currline" => {
                t.next();
            }
            "score" => match t.next() {
                Some("cp") => info.score = t.next().and_then(|v| v.parse().ok()).map(Score::Cp),
                Some("mate") => {
                    info.score = t.next().map(|v| match v {
                        "+" => Score::Mate(999),
                        "-" => Score::Mate(-999),
                        _ => Score::Mate(v.parse().unwrap_or(999)),
                    })
                }
                _ => {}
            },
            "lowerbound" | "upperbound" => info.bound = Some(key.to_owned()),
            "pv" => {
                info.pv = t.by_ref().map(str::to_owned).collect();
            }
            "string" => {
                info.string = Some(t.by_ref().collect::<Vec<_>>().join(" "));
            }
            _ => {}
        }
    }
    Some(info)
}

/// Builds a `go` command.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GoParams {
    pub btime: Option<u64>,
    pub wtime: Option<u64>,
    pub byoyomi: Option<u64>,
    pub binc: Option<u64>,
    pub winc: Option<u64>,
    pub nodes: Option<u64>,
    pub depth: Option<u32>,
    pub movetime: Option<u64>,
    pub infinite: bool,
}

impl GoParams {
    pub fn command(&self) -> String {
        let mut s = String::from("go");
        if self.infinite {
            s.push_str(" infinite");
            return s;
        }
        let mut push = |k: &str, v: Option<u64>| {
            if let Some(v) = v {
                s.push_str(&format!(" {k} {v}"));
            }
        };
        push("btime", self.btime);
        push("wtime", self.wtime);
        if self.binc.is_some() || self.winc.is_some() {
            push("binc", self.binc);
            push("winc", self.winc);
        } else {
            push("byoyomi", self.byoyomi);
        }
        push("nodes", self.nodes);
        push("depth", self.depth.map(u64::from));
        push("movetime", self.movetime);
        s
    }
}

#[derive(Debug)]
pub enum UsiError {
    Io(std::io::Error),
    Timeout(&'static str),
    Exited,
}

impl std::fmt::Display for UsiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsiError::Io(e) => write!(f, "engine I/O error: {e}"),
            UsiError::Timeout(w) => write!(f, "the engine did not answer ({w})"),
            UsiError::Exited => write!(f, "the engine exited"),
        }
    }
}

impl From<std::io::Error> for UsiError {
    fn from(e: std::io::Error) -> Self {
        UsiError::Io(e)
    }
}

fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

/// A running USI engine. Output lines arrive on a channel until the process exits.
pub struct UsiProcess {
    child: Child,
    stdin: ChildStdin,
    rx: Option<Receiver<String>>,
    /// Lines written to the engine and read back, for the log view.
    pub log: Option<mpsc::Sender<(bool, String)>>,
}

impl UsiProcess {
    pub fn spawn(path: &Path) -> Result<Self, UsiError> {
        let mut cmd = Command::new(path);
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or(UsiError::Exited)?;
        let stdout = child.stdout.take().ok_or(UsiError::Exited)?;
        let (tx, rx) = mpsc::channel();
        thread::Builder::new().name("usi-reader".into()).spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                // Bound each line: a misbehaving engine cannot make us buffer without limit.
                match (&mut reader).take(MAX_LINE as u64).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if !buf.ends_with(b"\n") {
                            // Over-long line: discard the rest of it.
                            let mut rest = Vec::new();
                            loop {
                                rest.clear();
                                match (&mut reader).take(MAX_LINE as u64).read_until(b'\n', &mut rest) {
                                    Ok(0) | Err(_) => break,
                                    Ok(_) if rest.ends_with(b"\n") => break,
                                    Ok(_) => {}
                                }
                            }
                        }
                        if tx.send(decode_line(&buf)).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
        Ok(UsiProcess { child, stdin, rx: Some(rx), log: None })
    }

    pub fn send(&mut self, line: &str) -> Result<(), UsiError> {
        if let Some(log) = &self.log {
            let _ = log.send((true, line.to_owned()));
        }
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn wait_for(&mut self, what: &'static str, timeout: Duration, mut f: impl FnMut(&str) -> bool) -> Result<(), UsiError> {
        let rx = self.rx.as_ref().ok_or(UsiError::Exited)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(line) => {
                    if let Some(log) = &self.log {
                        let _ = log.send((false, line.clone()));
                    }
                    if f(&line) {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Err(UsiError::Timeout(what)),
                Err(RecvTimeoutError::Disconnected) => return Err(UsiError::Exited),
            }
        }
    }

    /// `usi` → `id`/`option` lines → `usiok`.
    pub fn handshake(&mut self, timeout: Duration) -> Result<EngineId, UsiError> {
        self.send("usi")?;
        let mut id = EngineId::default();
        self.wait_for("usiok", timeout, |line| {
            if let Some(n) = line.strip_prefix("id name ") {
                id.name = n.trim().to_owned();
            } else if let Some(a) = line.strip_prefix("id author ") {
                id.author = a.trim().to_owned();
            } else if let Some(o) = parse_option(line) {
                id.options.push(o);
            }
            line.trim() == "usiok"
        })?;
        Ok(id)
    }

    pub fn set_option(&mut self, name: &str, value: &str) -> Result<(), UsiError> {
        if value.is_empty() {
            self.send(&format!("setoption name {name} value <empty>"))
        } else {
            self.send(&format!("setoption name {name} value {value}"))
        }
    }

    pub fn isready(&mut self, timeout: Duration) -> Result<(), UsiError> {
        self.send("isready")?;
        self.wait_for("readyok", timeout, |line| line.trim() == "readyok")
    }

    /// Wait for `bestmove`, returning (move, ponder) and the infos seen meanwhile.
    pub fn wait_bestmove(&mut self, timeout: Duration) -> Result<(String, Vec<Info>), UsiError> {
        let mut best = String::new();
        let mut infos = Vec::new();
        self.wait_for("bestmove", timeout, |line| {
            if let Some(i) = parse_info(line) {
                infos.push(i);
            }
            if let Some(m) = line.strip_prefix("bestmove") {
                best = m.split_whitespace().next().unwrap_or("").to_owned();
                return true;
            }
            false
        })?;
        Ok((best, infos))
    }

    /// Hand the output channel to the caller (e.g. a pump thread).
    pub fn take_receiver(&mut self) -> Option<Receiver<String>> {
        self.rx.take()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// `quit`, then kill if the engine does not exit within a grace period.
    pub fn quit(mut self) {
        let _ = self.send("quit");
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            if !self.is_running() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for UsiProcess {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Start an engine: handshake, apply option values, isready, usinewgame.
pub fn start_engine(
    path: &Path,
    values: &[(String, String)],
    ready_timeout: Duration,
) -> Result<(UsiProcess, EngineId), UsiError> {
    let mut p = UsiProcess::spawn(path)?;
    let id = p.handshake(Duration::from_secs(15))?;
    for (k, v) in values {
        p.set_option(k, v)?;
    }
    p.isready(ready_timeout)?;
    p.send("usinewgame")?;
    Ok((p, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let o = parse_option("option name Threads type spin default 4 min 1 max 1024").unwrap();
        assert_eq!((o.name.as_str(), o.kind.as_str(), o.default.as_str(), o.min, o.max), ("Threads", "spin", "4", Some(1), Some(1024)));
        let o = parse_option("option name BookFile type combo default standard_book.db var no_book var standard_book.db").unwrap();
        assert_eq!(o.vars, ["no_book", "standard_book.db"]);
        let o = parse_option("option name WriteDebugLog type string default ").unwrap();
        assert_eq!(o.default, "");
        let o = parse_option("option name Eval Dir type string default <empty>").unwrap();
        assert_eq!((o.name.as_str(), o.default.as_str()), ("Eval Dir", ""));
        let o = parse_option("option name Clear Hash type button").unwrap();
        assert_eq!(o.kind, "button");
        assert!(parse_option("info depth 1").is_none());
    }

    #[test]
    fn infos() {
        let i = parse_info("info depth 12 seldepth 18 score cp -35 upperbound multipv 2 nodes 123456 nps 987654 time 125 hashfull 12 pv 7g7f 3c3d 2g2f").unwrap();
        assert_eq!(i.depth, Some(12));
        assert_eq!(i.seldepth, Some(18));
        assert_eq!(i.score, Some(Score::Cp(-35)));
        assert_eq!(i.bound.as_deref(), Some("upperbound"));
        assert_eq!(i.multipv, Some(2));
        assert_eq!(i.nodes, Some(123456));
        assert_eq!(i.nps, Some(987654));
        assert_eq!(i.time_ms, Some(125));
        assert_eq!(i.pv, ["7g7f", "3c3d", "2g2f"]);
        assert_eq!(parse_info("info score mate -3 pv 5a4b").unwrap().score, Some(Score::Mate(-3)));
        assert_eq!(parse_info("info score mate + pv").unwrap().score, Some(Score::Mate(999)));
        assert_eq!(parse_info("info string hello world").unwrap().string.as_deref(), Some("hello world"));
        assert!(parse_info("bestmove 7g7f").is_none());
    }

    #[test]
    fn go_commands() {
        let g = GoParams { btime: Some(60000), wtime: Some(50000), byoyomi: Some(1000), ..Default::default() };
        assert_eq!(g.command(), "go btime 60000 wtime 50000 byoyomi 1000");
        let g = GoParams { btime: Some(1), wtime: Some(2), binc: Some(3), winc: Some(3), byoyomi: Some(9), ..Default::default() };
        assert_eq!(g.command(), "go btime 1 wtime 2 binc 3 winc 3");
        let g = GoParams { nodes: Some(1000), ..Default::default() };
        assert_eq!(g.command(), "go nodes 1000");
        assert_eq!(GoParams { infinite: true, ..Default::default() }.command(), "go infinite");
    }

    #[test]
    fn decode_engine_output() {
        assert_eq!(decode_line(b"id name test\r\n"), "id name test");
        let sjis = encoding_rs::SHIFT_JIS.encode("id name 将棋").0;
        assert_eq!(decode_line(&sjis), "id name 将棋");
    }

    /// Talk to a tiny scripted "engine" (a shell `cmd` script) to test the process plumbing.
    #[cfg(windows)]
    #[test]
    fn handshake_with_scripted_engine() {
        let dir = std::env::temp_dir().join(format!("usi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake.cmd");
        std::fs::write(
            &script,
            "@echo off\r\necho id name FakeEngine 1.0\r\necho id author Tester\r\necho option name Threads type spin default 1 min 1 max 8\r\necho option name BookFile type combo default a var a var b\r\necho usiok\r\necho readyok\r\necho info depth 3 score cp 42 pv 7g7f\r\necho bestmove 7g7f ponder 3c3d\r\n",
        )
        .unwrap();
        let mut p = UsiProcess::spawn(&script).unwrap();
        let id = p.handshake(Duration::from_secs(10)).unwrap();
        assert_eq!(id.name, "FakeEngine 1.0");
        assert_eq!(id.author, "Tester");
        assert_eq!(id.options.len(), 2);
        p.isready(Duration::from_secs(10)).unwrap();
        let (best, infos) = p.wait_bestmove(Duration::from_secs(10)).unwrap();
        assert_eq!(best, "7g7f");
        assert_eq!(infos[0].score, Some(Score::Cp(42)));
        p.quit();
        let _ = std::fs::remove_dir_all(dir);
    }
}
