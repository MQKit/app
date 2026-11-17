//! The tool list: static `plugin.json` manifests, read without starting any process.

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::RwLock;

pub struct Tool {
    pub id: String,
    pub manifest: Value,
}

static TOOLS: RwLock<BTreeMap<String, Value>> = RwLock::new(BTreeMap::new());

pub fn load() -> Vec<String> {
    let mut map = BTreeMap::new();
    let mut problems = Vec::new();
    if let Ok(rd) = std::fs::read_dir(crate::paths::plugins_dir()) {
        for e in rd.flatten() {
            let p = e.path().join("plugin.json");
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            match serde_json::from_str::<Value>(&text) {
                Ok(m) if m["protocol"].as_u64() == Some(contract::PROTOCOL as u64) => {
                    if let Some(id) = m["id"].as_str() {
                        map.insert(id.to_string(), m);
                    }
                }
                Ok(_) => problems.push(format!("{}: protocol", p.display())),
                Err(err) => problems.push(format!("{}: {err}", p.display())),
            }
        }
    }
    *TOOLS.write().unwrap() = map;
    problems
}

pub fn all() -> Vec<Value> {
    TOOLS.read().unwrap().values().cloned().collect()
}

pub fn get(id: &str) -> Option<Tool> {
    TOOLS.read().unwrap().get(id).map(|m| Tool { id: id.to_string(), manifest: m.clone() })
}

/// How to start a plugin for a given form and batch.
pub struct Launch {
    /// The entry's id (`word`, `libreoffice`, `pdf2docx`…), reported as `resolvedEngine`.
    pub id: String,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, std::ffi::OsString)>,
    /// Engines needed, requirements included.
    pub engines: Vec<String>,
}

impl Tool {
    /// Field defaults that are plain values (enough to choose an entry before the form exists).
    pub fn defaults(&self) -> Map<String, Value> {
        let mut m = Map::new();
        if let Some(fields) = self.manifest["fields"].as_array() {
            for f in fields {
                if let (Some(k), Some(d)) = (f["key"].as_str(), f.get("default")) {
                    if !d.is_array() {
                        m.insert(k.to_string(), d.clone());
                    }
                }
            }
        }
        m
    }

    /// Chooses the first entry whose `when` holds (contract 5.2), and the engines it needs.
    pub fn launch(&self, values: &Map<String, Value>, files: &[Map<String, Value>]) -> Option<Launch> {
        let mut values_all = self.defaults();
        for (k, v) in values {
            values_all.insert(k.clone(), v.clone());
        }
        let mut env = match crate::env::props("") {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        let entries = self.manifest["entry"].as_array()?;
        let entry = entries.iter().find(|e| {
            let ctx = contract::cond::Ctx { values: &values_all, files, env: &env };
            contract::cond::eval(e.get("when"), &ctx)
        })?;
        let id = entry["id"].as_str().unwrap_or("default").to_string();
        env.insert("resolvedEngine".into(), Value::from(id.clone()));
        let ctx = contract::cond::Ctx { values: &values_all, files, env: &env };
        let mut engines: Vec<String> = Vec::new();
        for r in self.manifest["requires"].as_array().cloned().unwrap_or_default() {
            if let Some(name) = r["engine"].as_str() {
                if contract::cond::eval(r.get("when"), &ctx) {
                    for n in crate::engines::with_requirements(name) {
                        if !engines.contains(&n) {
                            engines.push(n);
                        }
                    }
                }
            }
        }
        let mut extra_env = Vec::new();
        let (exe, args) = match entry["kind"].as_str()? {
            "worker" | "dotnet" => {
                let w = entry["worker"].as_str().unwrap_or("worker-core");
                let cmd = entry["command"].as_str().unwrap_or(&self.id);
                (crate::paths::worker_exe(w), vec![cmd.to_string()])
            }
            "python" => {
                if !engines.iter().any(|e| e == "python") {
                    engines.insert(0, "python".into());
                }
                let py = crate::env::engine_dir("python").map(|d| d.join("python.exe")).unwrap_or_else(|| PathBuf::from("python.exe"));
                let script = crate::paths::plugins_dir().join(&self.id).join(entry["script"].as_str()?);
                let site: Vec<PathBuf> = engines
                    .iter()
                    .filter(|e| crate::engines::mode(e).as_deref() == Some("wheels"))
                    .filter_map(|e| crate::env::engine_dir(e))
                    .map(|d| d.join("site-packages"))
                    .collect();
                if let Ok(p) = std::env::join_paths(site) {
                    extra_env.push(("CONVERTOR_PYPATH".to_string(), p));
                }
                extra_env.push(("PYTHONUTF8".to_string(), "1".into()));
                (py, vec!["-I".into(), "-X".into(), "utf8".into(), script.to_string_lossy().into_owned()])
            }
            _ => return None,
        };
        Some(Launch { id, exe, args, env: extra_env, engines })
    }

    /// The process for `analyze`: the manifest's `analyzeEntry` when it has one
    /// (an extension of contract 5.2), else the entry chosen for the default form.
    pub fn analyze_launch(&self) -> Option<Launch> {
        if let Some(e) = self.manifest.get("analyzeEntry").filter(|e| e["kind"] == "worker") {
            let w = e["worker"].as_str().unwrap_or("worker-core");
            let cmd = e["command"].as_str().unwrap_or(&self.id);
            return Some(Launch { id: "analysis".into(), exe: crate::paths::worker_exe(w), args: vec![cmd.to_string()], env: vec![], engines: vec![] });
        }
        self.launch(&Map::new(), &[])
    }

    pub fn resource_class(&self) -> &str {
        self.manifest["resources"]["class"].as_str().unwrap_or("light")
    }
}
