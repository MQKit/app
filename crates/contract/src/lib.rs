//! Contract v2 between the parent app and the plugins.
//! Shared by `worker-core` (plugin side) and the Tauri app (parent side).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub mod cond;
pub mod pages;

pub const PROTOCOL: u32 = 2;

/// Common error codes (section 13.1).
pub mod codes {
    pub const WRONG_PASSWORD: &str = "wrong_password";
    pub const PASSWORD_REQUIRED: &str = "password_required";
    pub const UNSUPPORTED_FILE: &str = "unsupported_file";
    pub const UNSUPPORTED_FEATURE: &str = "unsupported_feature";
    pub const CORRUPT_FILE: &str = "corrupt_file";
    pub const FILE_TOO_LARGE: &str = "file_too_large";
    pub const ENGINE_MISSING: &str = "engine_missing";
    pub const ENGINE_FAILED: &str = "engine_failed";
    pub const ENGINE_CRASHED: &str = "engine_crashed";
    pub const INVALID_OPTIONS: &str = "invalid_options";
    pub const TARGET_UNREACHABLE: &str = "target_unreachable";
    pub const OUTPUT_LARGER: &str = "output_larger";
    pub const RANGE_OUT_OF_BOUNDS: &str = "range_out_of_bounds";
    pub const DISK_FULL: &str = "disk_full";
    pub const PERMISSION_DENIED: &str = "permission_denied";
    pub const FILE_LOCKED: &str = "file_locked";
    pub const OUT_OF_MEMORY: &str = "out_of_memory";
    pub const OFFICE_NOT_READY: &str = "office_not_ready";
    pub const TIMEOUT: &str = "timeout";
    pub const CANCELLED: &str = "cancelled";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub id: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_path: Option<PathBuf>,
    #[serde(default)]
    pub analysis: Map<String, Value>,
    #[serde(default)]
    pub item: Map<String, Value>,
    /// The folder the user dropped, when the file came from one (for keeping structure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<PathBuf>,
}

impl FileRef {
    pub fn input_path(&self) -> &PathBuf {
        self.safe_path.as_ref().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineRef {
    pub path: PathBuf,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub threads: u32,
    pub memory_mb: u32,
    pub max_megapixels: u32,
    pub idle_timeout_sec: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { threads: 2, memory_mb: 2048, max_megapixels: 250, idle_timeout_sec: 120 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Secrets {
    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

/// Any request sent on stdin (`analyze`, `preview`, `run`, `probe`, `cancel`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<String>,
    #[serde(default)]
    pub files: Vec<FileRef>,
    /// `preview`: the file id the preview is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default)]
    pub options: Map<String, Value>,
    #[serde(default)]
    pub resolved: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    #[serde(default)]
    pub engines: HashMap<String, EngineRef>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub secrets: Secrets,
}

impl Request {
    pub fn opt_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }
    pub fn opt_f64(&self, key: &str) -> Option<f64> {
        self.options.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
    }
    pub fn opt_bool(&self, key: &str) -> Option<bool> {
        self.options.get(key).and_then(Value::as_bool)
    }
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.opt_str(key).unwrap_or(default)
    }
    pub fn f64_or(&self, key: &str, default: f64) -> f64 {
        self.opt_f64(key).unwrap_or(default)
    }
    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.opt_bool(key).unwrap_or(default)
    }
}

/// Writes JSON lines on stdout, flushed after each line.
#[derive(Clone)]
pub struct Emitter {
    inner: Arc<Mutex<EmitState>>,
}

struct EmitState {
    last_emit: std::time::Instant,
    last_progress: Option<Value>,
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Emitter {
    pub fn new() -> Self {
        Emitter { inner: Arc::new(Mutex::new(EmitState { last_emit: std::time::Instant::now(), last_progress: None })) }
    }
    pub fn emit(&self, msg: Value) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.last_emit = std::time::Instant::now();
        match msg.get("type").and_then(Value::as_str) {
            Some("progress") => g.last_progress = Some(msg.clone()),
            Some("fileDone") | Some("fileStart") => g.last_progress = None,
            _ => {}
        }
        let mut out = std::io::stdout().lock();
        let _ = serde_json::to_writer(&mut out, &msg);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
    /// Heartbeat: while a long step runs without messages (one big encode, an engine
    /// that prints nothing), re-sends the last progress every `every`, so the parent
    /// knows the plugin is alive (contract 12.4) and the UI never looks frozen.
    pub fn start_heartbeat(&self, every: std::time::Duration) {
        let e = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(every / 4);
            let msg = {
                let g = e.inner.lock().unwrap_or_else(|x| x.into_inner());
                if g.last_emit.elapsed() < every {
                    continue;
                }
                g.last_progress.clone()
            };
            if let Some(mut m) = msg {
                m["heartbeat"] = json!(true);
                e.emit(m);
            }
        });
    }

    pub fn hello(&self, plugin: &str, version: &str) {
        self.emit(json!({"type":"hello","protocol":PROTOCOL,"plugin":plugin,"pluginVersion":version}));
    }
    pub fn file_start(&self, file: &str) {
        self.emit(json!({"type":"fileStart","file":file}));
    }
    pub fn progress(&self, file: &str, fraction: f64, stage_code: &str, stage: u32, stages: u32, detail: Option<Value>) {
        let mut m = json!({"type":"progress","file":file,"fraction":fraction.clamp(0.0, 1.0),"stageCode":stage_code,"stage":stage,"stages":stages});
        if let Some(d) = detail {
            m["detail"] = d;
        }
        self.emit(m);
    }
    pub fn warning(&self, file: Option<&str>, code: &str, details: Option<Value>) {
        let mut m = json!({"type":"warning","code":code});
        if let Some(f) = file {
            m["file"] = json!(f);
        }
        if let Some(d) = details {
            m["details"] = d;
        }
        self.emit(m);
    }
    pub fn output(&self, file: &str, path: &std::path::Path, ext: &str, vars: Value, stats: Value) {
        self.emit(json!({"type":"output","file":file,"path":path,"ext":ext,"vars":vars,"role":"main","stats":stats}));
    }
    pub fn done(&self, id: Option<&str>, summary: Option<Value>, cancelled: bool) {
        let mut m = json!({"type":"done"});
        if let Some(i) = id {
            m["id"] = json!(i);
        }
        if let Some(s) = summary {
            m["summary"] = s;
        }
        if cancelled {
            m["cancelled"] = json!(true);
        }
        self.emit(m);
    }
    pub fn error(&self, code: &str, details: Value) {
        self.emit(json!({"type":"error","code":code,"details":details}));
    }
}

/// Cancellation flags, set by a thread that keeps reading stdin.
#[derive(Clone, Default)]
pub struct Cancel {
    now: Arc<AtomicBool>,
    after_current: Arc<AtomicBool>,
}

impl Cancel {
    pub fn is_cancelled(&self) -> bool {
        self.now.load(Ordering::Relaxed)
    }
    pub fn stop_after_current(&self) -> bool {
        self.after_current.load(Ordering::Relaxed) || self.is_cancelled()
    }
    pub fn cancel(&self) {
        self.now.store(true, Ordering::Relaxed);
    }
    /// Starts the stdin watcher. Must be called after the first request line was read.
    pub fn watch_stdin(&self) {
        let c = self.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            if v.get("cmd").and_then(Value::as_str) == Some("cancel") {
                                if v.get("after").and_then(Value::as_str) == Some("currentFile") {
                                    c.after_current.store(true, Ordering::Relaxed);
                                } else {
                                    c.now.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}

/// A failure for one file: code plus optional details and retry suggestion.
#[derive(Debug, Clone)]
pub struct FileError {
    pub code: String,
    pub details: Option<Value>,
    pub suggest: Option<Value>,
    /// `skipped` instead of `failed` (for example `output_larger`).
    pub skipped: bool,
}

impl FileError {
    pub fn new(code: &str) -> Self {
        FileError { code: code.to_string(), details: None, suggest: None, skipped: false }
    }
    pub fn with(code: &str, details: Value) -> Self {
        FileError { code: code.to_string(), details: Some(details), suggest: None, skipped: false }
    }
    pub fn skipped(code: &str, details: Value) -> Self {
        FileError { code: code.to_string(), details: Some(details), suggest: None, skipped: true }
    }
    pub fn suggest(mut self, options: Value) -> Self {
        self.suggest = Some(json!({ "options": options }));
        self
    }
    pub fn engine(msg: impl std::fmt::Display) -> Self {
        Self::with(codes::ENGINE_FAILED, json!({ "message": msg.to_string() }))
    }
}

impl<E: std::error::Error> From<E> for FileError {
    fn from(e: E) -> Self {
        FileError::engine(e)
    }
}

/// Localized text: `{ "ro": "...", "en": "..." }`.
pub fn t(ro: &str, en: &str) -> Value {
    json!({ "ro": ro, "en": en })
}

/// Option of a `select` field.
pub fn opt(value: impl Into<Value>, ro: &str, en: &str) -> Value {
    json!({ "value": value.into(), "label": t(ro, en) })
}
