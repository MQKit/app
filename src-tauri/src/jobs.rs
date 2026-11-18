//! Running a job: work folder, worker process, messages, final names, moves,
//! originals, cancellation and timeouts (contract sections 8.3, 11 and 12).

use crate::{naming, paths, registry, worker};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobFile {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub analysis: Map<String, Value>,
    #[serde(default)]
    pub item: Map<String, Value>,
    #[serde(default)]
    pub base: Option<PathBuf>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OutputBlock {
    #[serde(default = "same")]
    pub output_dir: String,
    #[serde(default)]
    pub subfolder_name: String,
    #[serde(default)]
    pub output_dir_path: Option<PathBuf>,
    #[serde(default)]
    pub name_template: String,
    #[serde(default = "rename")]
    pub on_existing: String,
    #[serde(default = "keep")]
    pub originals: String,
    #[serde(default)]
    pub keep_file_date: bool,
}

fn same() -> String {
    "sameAsInput".into()
}
fn rename() -> String {
    "rename".into()
}
fn keep() -> String {
    "keep".into()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartJob {
    pub tool: String,
    pub tool_name: String,
    pub files: Vec<JobFile>,
    #[serde(default)]
    pub options: Map<String, Value>,
    pub output: OutputBlock,
    #[serde(default)]
    pub passwords: HashMap<String, String>,
    #[serde(default)]
    pub locale: String,
}

struct Control {
    cancel: AtomicBool,
    after_current: AtomicBool,
}

static JOBS: Mutex<Option<HashMap<String, Arc<Control>>>> = Mutex::new(None);

pub fn cancel(job: &str, after_current: bool) {
    if let Some(map) = JOBS.lock().unwrap().as_ref() {
        if let Some(c) = map.get(job) {
            if after_current {
                c.after_current.store(true, Ordering::Relaxed);
            } else {
                c.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn new_job_id() -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    format!("j{t}-{}", N.fetch_add(1, Ordering::Relaxed))
}

pub fn start(job: StartJob, events: Channel<Value>) -> Result<String, String> {
    let tool = registry::get(&job.tool).ok_or("unknown tool")?;
    if job.files.is_empty() {
        return Err("no files".into());
    }
    let id = new_job_id();
    let ctl = Arc::new(Control { cancel: AtomicBool::new(false), after_current: AtomicBool::new(false) });
    JOBS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.clone(), ctl.clone());
    let jid = id.clone();
    std::thread::spawn(move || {
        let summary = run(&jid, &tool, &job, &ctl, &events);
        crate::state::add_history(json!({
            "jobId": jid, "tool": job.tool, "toolName": job.tool_name,
            "at": crate::state::now_iso(), "files": job.files.len(),
            "summary": summary,
        }));
        if let Some(m) = JOBS.lock().unwrap().as_mut() {
            m.remove(&jid);
        }
    });
    Ok(id)
}

struct Out {
    path: PathBuf,
    ext: String,
    vars: Map<String, Value>,
}

fn class_limits(class: &str) -> (u32, u64, u64) {
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    // (threads, memory MB, idle timeout s)
    match class {
        "heavy" => ((cores.saturating_sub(2)).max(1), 4096, 180),
        "office" => (1, 0, 300),
        "gpu" => (cores.max(2) / 2, 8192, 300),
        _ => ((cores / 2).max(1), 3072, 120),
    }
}

fn run(job_id: &str, tool: &registry::Tool, job: &StartJob, ctl: &Control, ev: &Channel<Value>) -> Value {
    let send = |v: Value| {
        let _ = ev.send(v);
    };
    send(json!({ "type": "started", "jobId": job_id }));

    let first_dest = naming::output_dir(&job.output, &job.files[0].path, &job.tool_name);
    let _ = std::fs::create_dir_all(&first_dest);
    let jdir = paths::job_dir(&first_dest, job_id);
    let work = jdir.join("work");
    let out = jdir.join("out");
    let _ = std::fs::create_dir_all(&work);
    let _ = std::fs::create_dir_all(&out);

    let class = tool.resource_class().to_string();
    let (threads, memory_mb, idle) = class_limits(&class);
    let analyses: Vec<Map<String, Value>> = job.files.iter().map(|f| f.analysis.clone()).collect();
    let Some(launch) = tool.launch(&job.options, &analyses) else {
        let summary = json!({ "ok": 0, "skipped": 0, "failed": job.files.len() });
        send(json!({ "type": "done", "summary": summary, "cancelled": false, "error": { "code": "engine_missing" } }));
        return summary;
    };
    let mut engines = Map::new();
    for e in &launch.engines {
        if let Some(d) = crate::env::engine_dir(e) {
            engines.insert(e.clone(), json!({ "path": d, "version": "" }));
        }
    }
    let files: Vec<Value> = job
        .files
        .iter()
        .map(|f| json!({ "id": f.id, "path": f.path, "safePath": f.path, "analysis": f.analysis, "item": f.item, "base": f.base }))
        .collect();
    let req = json!({
        "cmd": "run", "protocol": contract::PROTOCOL, "jobId": job_id, "locale": job.locale,
        "files": files, "options": job.options, "resolved": { "engine": launch.id },
        "workDir": work, "outDir": out, "engines": engines,
        "limits": { "threads": threads, "memoryMb": memory_mb, "maxMegapixels": 250, "idleTimeoutSec": idle },
        "secrets": { "files": job.passwords, "fields": {} }
    });

    let mut counts = (0u32, 0u32, 0u32);
    let mut in_total = 0u64;
    let mut out_total = 0u64;
    let mut error: Option<Value> = None;
    let mut cancelled = false;

    match worker::spawn(tool, &launch, "run", Some(&work), memory_mb) {
        Err(e) => error = Some(json!({ "code": "engine_missing", "details": { "message": e.to_string() } })),
        Ok((mut proc, stdout)) => {
            proc.send(&req);
            let (tx, rx) = mpsc::channel::<Value>();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    match serde_json::from_str::<Value>(&line) {
                        Ok(v) => {
                            if tx.send(v).is_err() {
                                break;
                            }
                        }
                        Err(_) => applog!("protocol: {line}"),
                    }
                }
            });
            let mut outputs: HashMap<String, Vec<Out>> = HashMap::new();
            let mut started: HashSet<String> = HashSet::new();
            let mut finished: HashSet<String> = HashSet::new();
            let mut office_pids: Vec<u32> = Vec::new();
            let mut used_names: HashSet<PathBuf> = HashSet::new();
            let inputs: HashSet<PathBuf> = job.files.iter().map(|f| naming::key(&f.path)).collect();
            let mut last = Instant::now();
            let mut cancel_sent: Option<Instant> = None;
            let mut after_sent = false;
            let mut got_done = false;
            loop {
                if ctl.cancel.load(Ordering::Relaxed) && cancel_sent.is_none() {
                    proc.send(&json!({ "cmd": "cancel" }));
                    cancel_sent = Some(Instant::now());
                    cancelled = true;
                }
                if ctl.after_current.load(Ordering::Relaxed) && !after_sent {
                    proc.send(&json!({ "cmd": "cancel", "after": "currentFile" }));
                    after_sent = true;
                }
                if let Some(t) = cancel_sent {
                    if t.elapsed() > Duration::from_secs(3) {
                        proc.kill();
                        crate::winutil::kill_office(&office_pids);
                        break;
                    }
                }
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(m) => {
                        last = Instant::now();
                        let file = m["file"].as_str().unwrap_or("").to_string();
                        match m["type"].as_str().unwrap_or("") {
                            "fileStart" => {
                                started.insert(file.clone());
                                send(m);
                            }
                            "progress" | "warning" | "partial" => send(m),
                            "childProcess" => {
                                if let Some(p) = m["pid"].as_u64() {
                                    office_pids.push(p as u32);
                                    if let Some(t) = crate::winutil::process_started(p as u32) {
                                        crate::state::office_started(p as u32, t);
                                    }
                                }
                            }
                            "output" => {
                                let path = PathBuf::from(m["path"].as_str().unwrap_or(""));
                                // Results must come from outDir (no `..` stepping out of it).
                                if path.starts_with(&out) && !path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                                    outputs.entry(file).or_default().push(Out {
                                        path,
                                        ext: m["ext"].as_str().unwrap_or("").to_string(),
                                        vars: m["vars"].as_object().cloned().unwrap_or_default(),
                                    });
                                }
                            }
                            "fileDone" => {
                                finished.insert(file.clone());
                                let status = m["status"].as_str().unwrap_or("failed");
                                if status == "ok" {
                                    let outs = outputs.remove(&file).unwrap_or_default();
                                    match finalize(job, &file, outs, &inputs, &mut used_names) {
                                        Ok((results, ib, ob)) => {
                                            counts.0 += 1;
                                            in_total += ib;
                                            out_total += ob;
                                            send(json!({ "type": "fileDone", "file": file, "status": "ok", "results": results, "resolved": m["resolved"] }));
                                        }
                                        Err(code) => {
                                            counts.2 += 1;
                                            send(json!({ "type": "fileDone", "file": file, "status": "failed", "code": code }));
                                        }
                                    }
                                } else {
                                    if status == "skipped" {
                                        counts.1 += 1;
                                    } else {
                                        counts.2 += 1;
                                    }
                                    send(m);
                                }
                            }
                            "done" => {
                                got_done = true;
                                if m["cancelled"].as_bool() == Some(true) {
                                    cancelled = true;
                                }
                                break;
                            }
                            "error" => {
                                error = Some(json!({ "code": m["code"], "details": m["details"] }));
                            }
                            _ => {}
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if last.elapsed() > Duration::from_secs(idle) {
                            proc.kill();
                            crate::winutil::kill_office(&office_pids);
                            error = Some(json!({ "code": "timeout" }));
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // Crash: a file that started but never finished.
            if !got_done && error.is_none() && !cancelled {
                for f in started.difference(&finished) {
                    counts.2 += 1;
                    send(json!({ "type": "fileDone", "file": f, "status": "failed", "code": "engine_crashed" }));
                }
                if started.is_empty() {
                    error = Some(json!({ "code": "engine_crashed" }));
                }
            }
            proc.stdin = None;
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = proc.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            proc.kill();
            // Office started for this job and still running (the worker crashed, was stopped,
            // or ended with an error before closing it): stopped, never left holding a document.
            crate::winutil::kill_office(&office_pids);
            crate::state::office_gone(&office_pids);
        }
    }
    paths::remove_tree(&jdir);
    if let Some(parent) = jdir.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    let summary = json!({ "ok": counts.0, "skipped": counts.1, "failed": counts.2, "inputBytes": in_total, "outputBytes": out_total });
    send(json!({ "type": "done", "summary": summary, "cancelled": cancelled, "error": error }));
    summary
}

/// Moves one file's results to their final place. Returns (results, inputBytes, outputBytes).
fn finalize(job: &StartJob, file: &str, outs: Vec<Out>, inputs: &HashSet<PathBuf>, used: &mut HashSet<PathBuf>) -> Result<(Vec<Value>, u64, u64), String> {
    let input = if file == "*" {
        job.files.first()
    } else {
        job.files.iter().find(|f| f.id == file)
    }
    .ok_or("unknown file")?;
    let in_bytes: u64 = if file == "*" {
        job.files.iter().map(|f| std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0)).sum()
    } else {
        std::fs::metadata(&input.path).map(|m| m.len()).unwrap_or(0)
    };
    let multi = outs.len() > 1;
    let replace = job.output.originals == "replace" && file != "*";
    let mut results = Vec::new();
    let mut out_bytes = 0;
    let mut written = 0;
    let mut original_kept: Option<&str> = None;
    for (i, o) in outs.iter().enumerate() {
        // A folder result (an unpacked archive): moved whole, never over an existing folder.
        if o.ext == "folder" {
            if !o.path.is_dir() {
                return Err("engine_failed".into());
            }
            let dir = naming::output_dir(&job.output, &input.path, &job.tool_name);
            std::fs::create_dir_all(&dir).map_err(|_| "permission_denied")?;
            let mut vars = o.vars.clone();
            vars.entry("n".to_string()).or_insert(json!(i + 1));
            let name = naming::render(&job.output.name_template, &input.path, &vars, multi);
            let mut target = dir.join(&name);
            if target.exists() || used.contains(&naming::key(&target)) {
                if job.output.on_existing == "skip" {
                    results.push(json!({ "skipped": true, "path": target, "reason": "exists" }));
                    continue;
                }
                target = naming::unique_dir(&target, used);
            }
            move_dir(&o.path, &target).map_err(|_| "permission_denied")?;
            crate::winutil::copy_internet_mark(&input.path, &target);
            let size = dir_size(&target);
            used.insert(naming::key(&target));
            out_bytes += size;
            results.push(json!({ "path": target, "name": target.file_name().map(|n| n.to_string_lossy().to_string()), "bytes": size, "folder": true }));
            continue;
        }
        let size = std::fs::metadata(&o.path).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            return Err("engine_failed".into());
        }
        let dir = if replace { input.path.parent().unwrap_or(Path::new(".")).to_path_buf() } else { naming::output_dir(&job.output, &input.path, &job.tool_name) };
        std::fs::create_dir_all(&dir).map_err(|_| "permission_denied")?;
        let template = if replace { "{name}".to_string() } else { job.output.name_template.clone() };
        let mut vars = o.vars.clone();
        vars.entry("n".to_string()).or_insert(json!(i + 1));
        let stem = naming::render(&template, &input.path, &vars, multi);
        let mut target = dir.join(format!("{stem}.{}", o.ext));
        let mut from = o.path.clone();

        if replace && i == 0 {
            // The result first goes beside the original under a temporary name, so that at
            // no moment is neither on disk; only then does the original go to the Recycle
            // Bin. On a drive without one (USB, network), or when Windows can't recycle it,
            // the original stays and the result gets a name of its own.
            let staged = dir.join(format!(".{stem}.mqkit-{}.part", std::process::id()));
            let _ = std::fs::remove_file(&staged);
            if !crate::winutil::move_new(&o.path, &staged).unwrap_or(false) {
                return Err("permission_denied".into());
            }
            from = staged;
            if !crate::winutil::has_recycle_bin(&input.path) {
                original_kept = Some("no_recycle_bin");
            } else if crate::winutil::recycle(&input.path).is_err() {
                original_kept = Some("file_locked");
            }
        }
        let k = naming::key(&target);
        let exists = target.exists() || used.contains(&k);
        // An input of this job is never overwritten, unless it is the original being
        // replaced, already in the Recycle Bin.
        let is_input = inputs.contains(&k) && !(replace && original_kept.is_none() && k == naming::key(&input.path));
        let mut overwrite = false;
        if exists || is_input {
            match job.output.on_existing.as_str() {
                "overwrite" if !is_input && !used.contains(&k) => overwrite = true,
                "skip" if !is_input => {
                    let _ = std::fs::remove_file(&from);
                    results.push(json!({ "skipped": true, "path": target, "reason": "exists" }));
                    continue;
                }
                _ => target = naming::unique(&target, used),
            }
        }
        target = place(&from, target, overwrite, used).map_err(|_| "permission_denied")?;
        if job.output.keep_file_date || replace {
            if let Ok(m) = std::fs::metadata(&input.path).and_then(|m| m.modified()) {
                if let Ok(f) = std::fs::File::options().write(true).open(&target) {
                    let _ = f.set_modified(m);
                }
            }
        }
        used.insert(naming::key(&target));
        out_bytes += size;
        written += 1;
        let mut r = json!({ "path": target, "name": target.file_name().map(|n| n.to_string_lossy().to_string()), "bytes": size });
        if let Some(why) = original_kept.filter(|_| i == 0) {
            r["originalKept"] = json!(why);
        }
        results.push(r);
    }
    // "Move originals to the Recycle Bin": only when something new was written, and never
    // deleting for good (a drive without a bin keeps them).
    if job.output.originals == "recycle" && file != "*" && written > 0 {
        let why = if !crate::winutil::has_recycle_bin(&input.path) {
            Some("no_recycle_bin")
        } else if crate::winutil::recycle(&input.path).is_err() {
            Some("file_locked")
        } else {
            None
        };
        if let (Some(why), Some(first)) = (why, results.iter_mut().find(|r| r["skipped"] != true)) {
            first["originalKept"] = json!(why);
        }
    }
    Ok((results, in_bytes, out_bytes))
}

/// A folder moved into place: renamed when on the same drive, else copied and removed.
fn move_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_dir(from, to)?;
    let _ = std::fs::remove_dir_all(from);
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_dir(&e.path(), &to.join(e.file_name()))?;
        } else if ft.is_file() {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Moves a finished result to `target`: over an existing file only when the person chose
/// Overwrite, else to the next free name if `target` was taken meanwhile (another job,
/// another program). Returns where it went.
fn place(from: &Path, mut target: PathBuf, overwrite: bool, used: &HashSet<PathBuf>) -> std::io::Result<PathBuf> {
    if overwrite {
        move_file(from, &target)?;
        return Ok(target);
    }
    for _ in 0..50 {
        if crate::winutil::move_new(from, &target)? {
            return Ok(target);
        }
        target = naming::unique(&target, used);
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}
