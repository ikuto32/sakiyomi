//! Registered USI engines and their per-engine option values, persisted as JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::protocol::{EngineId, OptionDef, UsiProcess};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineConfig {
    pub id: String,
    /// Display name (editable); defaults to the engine's `id name`.
    pub name: String,
    pub path: String,
    pub author: String,
    pub usi_name: String,
    pub options: Vec<OptionDef>,
    /// Option values chosen by the user (missing = engine default).
    pub values: BTreeMap<String, String>,
}

impl EngineConfig {
    /// Values to send with `setoption`, in the engine's declared order.
    /// Only values that differ from the engine default are sent (engines such as
    /// YaneuraOu take `<empty>` literally), plus USI_Hash and USI_Ponder, which GUIs
    /// conventionally always send. USI_Ponder is forced off (the GUI does not ponder).
    pub fn effective_values(&self, overrides: &[(String, String)]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for o in &self.options {
            if o.kind == "button" {
                continue;
            }
            let mut v = self.values.get(&o.name).cloned().unwrap_or_else(|| o.default.clone());
            if let Some((_, ov)) = overrides.iter().find(|(k, _)| *k == o.name) {
                v = ov.clone();
            }
            if o.name == "USI_Ponder" {
                v = "false".into();
            }
            if v != o.default || o.name == "USI_Hash" || o.name == "USI_Ponder" {
                out.push((o.name.clone(), v));
            }
        }
        for (k, v) in overrides {
            if !out.iter().any(|(n, _)| n == k) && (k == "USI_Hash" || k == "MultiPV") {
                out.push((k.clone(), v.clone()));
            }
        }
        out
    }
}

pub struct Registry {
    file: PathBuf,
    engines: Mutex<Vec<EngineConfig>>,
}

impl Registry {
    pub fn load(file: PathBuf) -> Self {
        let engines = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Registry { file, engines: Mutex::new(engines) }
    }

    fn save(&self, engines: &[EngineConfig]) -> Result<(), String> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(engines).map_err(|e| e.to_string())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Vec<EngineConfig> {
        self.engines.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn get(&self, id: &str) -> Option<EngineConfig> {
        self.engines.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|e| e.id == id).cloned()
    }

    /// Launch the executable, read `id`/`option` lines, and store it.
    pub fn register(&self, path: &Path) -> Result<EngineConfig, String> {
        let probe = probe(path)?;
        let id = format!(
            "e{:x}",
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        );
        let cfg = EngineConfig {
            id,
            name: if probe.name.is_empty() { file_stem(path) } else { probe.name.clone() },
            path: path.to_string_lossy().into_owned(),
            author: probe.author,
            usi_name: probe.name,
            options: probe.options,
            values: BTreeMap::new(),
        };
        let mut list = self.engines.lock().unwrap_or_else(|e| e.into_inner());
        list.push(cfg.clone());
        self.save(&list)?;
        Ok(cfg)
    }

    /// Re-read options from the executable (e.g. after an engine update), keeping values.
    pub fn refresh(&self, id: &str) -> Result<EngineConfig, String> {
        let path = self.get(id).ok_or_else(|| "engine not found".to_owned())?.path;
        let probe = probe(Path::new(&path))?;
        let mut list = self.engines.lock().unwrap_or_else(|e| e.into_inner());
        let e = list.iter_mut().find(|e| e.id == id).ok_or_else(|| "engine not found".to_owned())?;
        e.options = probe.options;
        e.usi_name = probe.name;
        e.author = probe.author;
        let names: Vec<String> = e.options.iter().map(|o| o.name.clone()).collect();
        e.values.retain(|k, _| names.contains(k));
        let out = e.clone();
        self.save(&list)?;
        Ok(out)
    }

    pub fn update(&self, id: &str, name: Option<String>, values: BTreeMap<String, String>) -> Result<EngineConfig, String> {
        let mut list = self.engines.lock().unwrap_or_else(|e| e.into_inner());
        let e = list.iter_mut().find(|e| e.id == id).ok_or_else(|| "engine not found".to_owned())?;
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            e.name = n.trim().to_owned();
        }
        // Store only values that differ from the default, so engine updates keep new defaults.
        e.values = values
            .into_iter()
            .filter(|(k, v)| e.options.iter().any(|o| &o.name == k && &o.default != v))
            .collect();
        let out = e.clone();
        self.save(&list)?;
        Ok(out)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.engines.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|e| e.id != id);
        self.save(&list)
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn probe(path: &Path) -> Result<EngineId, String> {
    let mut p = UsiProcess::spawn(path).map_err(|e| format!("cannot start: {e}"))?;
    let id = p.handshake(Duration::from_secs(15)).map_err(|e| e.to_string());
    p.quit();
    id
}
