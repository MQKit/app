#[macro_use]
mod applog;
mod engines;
mod env;
mod find;
mod jobs;
mod link;
mod naming;
mod paths;
mod registry;
mod shell;
mod state;
mod update;
mod winutil;
mod worker;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};
use tauri::ipc::Channel;
use tauri::Manager;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileInfo {
    pub path: PathBuf,
    pub name: String,
    pub ext: String,
    pub size: u64,
    pub modified: u64,
    /// Folder the user dropped, for "keep folder structure".
    pub base: Option<PathBuf>,
}

pub(crate) fn file_info(p: &Path, base: Option<&Path>) -> Option<FileInfo> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() {
        return None;
    }
    Some(FileInfo {
        path: p.to_path_buf(),
        name: p.file_name()?.to_string_lossy().to_string(),
        ext: p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default(),
        size: m.len(),
        modified: m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0),
        base: base.map(Path::to_path_buf),
    })
}

fn walk(dir: &Path, base: &Path, depth: u32, out: &mut Vec<FileInfo>) {
    if depth > 8 || out.len() >= 5000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db") {
            continue;
        }
        if ft.is_dir() {
            walk(&e.path(), base, depth + 1, out);
        } else if let Some(fi) = file_info(&e.path(), Some(base)) {
            out.push(fi);
        }
    }
}

/// Files and folders (expanded) → file list. Images are allowed for thumbnails.
/// Files and folders dropped or picked, folders walked: off the window's thread (a
/// folder of thousands of files on a network share would freeze it).
#[tauri::command]
async fn expand_paths(app: tauri::AppHandle, paths: Vec<PathBuf>) -> Vec<FileInfo> {
    tauri::async_runtime::spawn_blocking(move || expand(&app, paths)).await.unwrap_or_default()
}

fn expand(app: &tauri::AppHandle, paths: Vec<PathBuf>) -> Vec<FileInfo> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(&p, &p, 0, &mut out);
        } else if let Some(fi) = file_info(&p, None) {
            out.push(fi);
        }
    }
    let scope = app.asset_protocol_scope();
    for f in &out {
        if matches!(f.ext.as_str(), "jpg" | "jpeg" | "jfif" | "png" | "webp" | "gif" | "bmp" | "ico" | "avif") {
            let _ = scope.allow_file(&f.path);
        }
    }
    out
}

/// "From a folder…": the files under `root` that match `pattern` (see find.rs) and that
/// the tool takes (`extensions`; empty = any file). Runs off the UI thread.
#[tauri::command]
async fn find_files(app: tauri::AppHandle, root: PathBuf, pattern: String, recursive: bool, hidden: bool, extensions: Vec<String>) -> Result<find::Found, String> {
    if !root.is_dir() {
        return Err("not_a_folder".into());
    }
    let found = tauri::async_runtime::spawn_blocking(move || find::find(&root, &pattern, recursive, hidden, &extensions))
        .await
        .map_err(|e| e.to_string())?;
    let scope = app.asset_protocol_scope();
    for f in &found.files {
        if matches!(f.ext.as_str(), "jpg" | "jpeg" | "jfif" | "png" | "webp" | "gif" | "bmp" | "ico" | "avif") {
            let _ = scope.allow_file(&f.path);
        }
    }
    Ok(found)
}

/// "From a link…": downloads into the Downloads folder; events: {type:"progress",done,total}
/// then {type:"done",file} or {type:"error",code,detail}.
#[tauri::command]
fn link_download(app: tauri::AppHandle, id: String, url: String, extensions: Vec<String>, on_event: Channel<Value>) {
    std::thread::spawn(move || {
        let ev = on_event.clone();
        let res = link::download(&id, &url, &extensions, &move |done, total| {
            let _ = ev.send(json!({ "type": "progress", "done": done, "total": total }));
        });
        let _ = match res {
            Ok(f) => {
                if matches!(f.ext.as_str(), "jpg" | "jpeg" | "jfif" | "png" | "webp" | "gif" | "bmp" | "ico" | "avif") {
                    let _ = app.asset_protocol_scope().allow_file(&f.path);
                }
                on_event.send(json!({ "type": "done", "file": f }))
            }
            Err(e) => on_event.send(json!({ "type": "error", "code": e.code, "detail": e.detail })),
        };
    });
}

#[tauri::command]
fn link_cancel(id: String) {
    link::cancel(&id);
}

#[tauri::command]
fn get_tools() -> Vec<Value> {
    registry::all()
}

#[tauri::command]
async fn get_env() -> Value {
    tauri::async_runtime::spawn_blocking(env_now).await.unwrap_or_default()
}

/// The PC's state for the tools' conditions (Office, voices, OCR languages, engines):
/// asking Windows takes a moment, so never on the window's thread.
fn env_now() -> Value {
    let lang = state::get("settings")["language"].as_str().map(String::from).unwrap_or_else(env::system_language);
    let mut v = env::props(&lang);
    v["systemLanguage"] = json!(env::system_language());
    v
}

#[tauri::command]
fn state_get(name: String) -> Value {
    state::get(&name)
}

#[tauri::command]
fn state_set(name: String, value: Value) -> Result<(), String> {
    state::set(&name, &value)
}

#[derive(Deserialize)]
struct AnalyzeFile {
    id: String,
    path: PathBuf,
}

type CacheKey = (String, PathBuf, u64, u64);
static ANALYSIS: Mutex<Option<HashMap<CacheKey, Value>>> = Mutex::new(None);

/// An answer given with a password isn't cached: a wrong one would be answered again for
/// the right one ("wrong password" until the app restarted).
fn cache_key(tool: &str, p: &Path, with_pw: bool) -> Option<CacheKey> {
    if with_pw {
        return None;
    }
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
    Some((tool.to_string(), p.to_path_buf(), m.len(), t))
}

/// Fast analysis (contract 9): cached by path + size + date. Passwords stay in memory only.
#[tauri::command]
async fn analyze(tool: String, files: Vec<AnalyzeFile>, passwords: Option<HashMap<String, String>>) -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let t = registry::get(&tool).ok_or("unknown tool")?;
        let passwords = passwords.unwrap_or_default();
        let mut results: HashMap<String, Value> = HashMap::new();
        let mut todo = Vec::new();
        let mut missing_code = "analysis_failed";
        {
            let cache = ANALYSIS.lock().unwrap();
            for f in &files {
                let key = cache_key(&tool, &f.path, passwords.contains_key(&f.id));
                match key.as_ref().and_then(|k| cache.as_ref().and_then(|c| c.get(k))) {
                    Some(v) => {
                        // The cached answer names the file by the id it had then: renamed to
                        // this request's id, or the screen never matched it ("analyzing…").
                        let mut v = v.clone();
                        v["file"] = json!(f.id);
                        results.insert(f.id.clone(), v);
                    }
                    None => todo.push(f),
                }
            }
        }
        if !todo.is_empty() {
            let req = json!({
                "cmd": "analyze", "id": "a", "level": "fast",
                "files": todo.iter().map(|f| json!({ "id": f.id, "path": f.path })).collect::<Vec<_>>(),
                "secrets": { "files": passwords, "fields": {} }
            });
            let timeout = Duration::from_secs(20 + 5 * todo.len() as u64);
            let launch = t.analyze_launch().ok_or("engine_missing")?;
            let mut req = req;
            req["engines"] = engines_json(&launch);
            let (msgs, timed_out) = worker::request_partial(&t, &launch, "analyze", &req, timeout)?;
            // A file the worker didn't answer: why, so the screen can say it and offer a retry.
            missing_code = if timed_out { "analysis_timeout" } else { "analysis_failed" };
            let mut cache = ANALYSIS.lock().unwrap();
            let cache = cache.get_or_insert_with(HashMap::new);
            for m in msgs {
                if m["type"] != "analysis" {
                    continue;
                }
                let id = m["file"].as_str().unwrap_or("").to_string();
                if let Some(f) = todo.iter().find(|f| f.id == id) {
                    if m["status"] != "failed" {
                        if let Some(k) = cache_key(&tool, &f.path, passwords.contains_key(&f.id)) {
                            cache.insert(k, m.clone());
                        }
                    }
                }
                results.insert(id, m);
            }
        }
        Ok(files
            .iter()
            .map(|f| results.remove(&f.id).unwrap_or_else(|| json!({ "file": f.id, "status": "failed", "code": missing_code, "props": {} })))
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn engines_json(l: &registry::Launch) -> Value {
    let mut m = Map::new();
    for e in &l.engines {
        if let Some(d) = env::engine_dir(e) {
            m.insert(e.clone(), json!({ "path": d, "version": "" }));
        }
    }
    Value::Object(m)
}

#[derive(Deserialize)]
struct EstimateFile {
    id: String,
    path: PathBuf,
    #[serde(default)]
    analysis: Map<String, Value>,
}

#[tauri::command]
async fn estimate(tool: String, files: Vec<EstimateFile>, options: Map<String, Value>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let t = registry::get(&tool).ok_or("unknown tool")?;
        let req = json!({
            "cmd": "preview", "id": "e", "target": { "kind": "estimate" }, "options": options,
            "files": files.iter().map(|f| json!({ "id": f.id, "path": f.path, "analysis": f.analysis })).collect::<Vec<_>>(),
        });
        let analyses: Vec<Map<String, Value>> = files.iter().map(|f| f.analysis.clone()).collect();
        let launch = t.launch(&options, &analyses).ok_or("engine_missing")?;
        let mut req = req;
        req["engines"] = engines_json(&launch);
        let msgs = worker::request(&t, &launch, "preview", &req, Duration::from_secs(30))?;
        Ok(msgs.into_iter().find(|m| m["type"] == "estimate").unwrap_or(Value::Null))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Before/after crop at 100 % (contract 10). The images go to `tmp\preview` and are
/// allowed for the asset protocol.
#[tauri::command]
async fn preview_image(app: tauri::AppHandle, tool: String, file: EstimateFile, options: Map<String, Value>, rect: Option<Vec<f64>>, seq: u64, max_px: Option<u32>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let t = registry::get(&tool).ok_or("unknown tool")?;
        let dir = paths::data_dir().join("tmp").join("preview");
        let _ = std::fs::create_dir_all(&dir);
        let mut target = json!({ "kind": "crop", "maxPx": max_px.unwrap_or(480).clamp(64, 1600) });
        if let Some(r) = rect {
            target["rect"] = json!(r);
        }
        let analyses = vec![file.analysis.clone()];
        let launch = t.launch(&options, &analyses).ok_or("engine_missing")?;
        let req = json!({
            "cmd": "preview", "id": format!("p{seq}"), "file": file.id, "target": target, "options": options,
            "previewDir": dir, "engines": engines_json(&launch),
            "files": [ { "id": file.id, "path": file.path, "analysis": file.analysis } ],
        });
        let msgs = worker::request(&t, &launch, "preview", &req, Duration::from_secs(30))?;
        let m = msgs.into_iter().find(|m| m["type"] == "preview" || m["type"] == "previewError").unwrap_or(Value::Null);
        let scope = app.asset_protocol_scope();
        for k in ["before", "path"] {
            if let Some(p) = m[k].as_str() {
                let _ = scope.allow_file(p);
            }
        }
        Ok(m)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn start_job(job: jobs::StartJob, on_event: Channel<Value>) -> Result<String, String> {
    jobs::start(job, on_event)
}

#[tauri::command]
fn cancel_job(job_id: String, after_current: Option<bool>) {
    jobs::cancel(&job_id, after_current.unwrap_or(false));
}

#[tauri::command]
async fn engines_list() -> Value {
    tauri::async_runtime::spawn_blocking(|| json!(engines::list())).await.unwrap_or_default()
}

/// Downloads an engine; events: {type:"progress",done,total} then {type:"done"} or {type:"error",code}.
#[tauri::command]
fn engine_download(name: String, on_event: Channel<Value>) {
    std::thread::spawn(move || {
        let ev = on_event.clone();
        let res = engines::download(&name, move |done, total| {
            let _ = ev.send(json!({ "type": "progress", "done": done, "total": total }));
        });
        let _ = match res {
            Ok(_) => on_event.send(json!({ "type": "done" })),
            Err(code) => on_event.send(json!({ "type": "error", "code": code })),
        };
    });
}

#[tauri::command]
fn engine_cancel(name: String) {
    engines::cancel(&name);
}

#[tauri::command]
fn engine_remove(name: String) -> Result<(), String> {
    engines::remove(&name)
}

/// Opens a result in its program. Only files and folders that exist, never a program or
/// a script (a result never is one): anything runnable is shown in its folder instead.
#[tauri::command]
fn open_path(path: PathBuf) -> Result<(), String> {
    const RUNNABLE: &[&str] = &[
        "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "scr", "pif", "hta", "cpl", "jar", "lnk", "url", "reg", "inf", "appref-ms", "application", "msc", "gadget", "dll", "sys",
    ];
    if !path.exists() {
        return Err("not_found".into());
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if path.is_file() && RUNNABLE.contains(&ext.as_str()) {
        return tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string());
    }
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal(path: PathBuf) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_update() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(update::check).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    // Web pages, the Store, and the Windows Settings pages tools point to.
    const SETTINGS: &[&str] = &["ms-settings:speech", "ms-settings:regionlanguage"];
    if !(url.starts_with("https://") || url.starts_with("ms-windows-store://") || SETTINGS.contains(&url.as_str())) {
        return Err("not allowed".into());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// HTTPS as Windows checks it (its certificate store, and the intermediate certificates
/// a server leaves out fetched as browsers do): for engine downloads, links, the version check.
pub(crate) fn tls() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build()
}

/// Files from other starts (Explorer starts one per selected file) that arrived before
/// the interface was listening: kept here until it asks for them. `None` until then;
/// after that, files go straight to the interface as `open-files` events.
static SHELL_QUEUE: Mutex<Option<Vec<Value>>> = Mutex::new(Some(Vec::new()));

/// Hands files from another start to the interface, or keeps them until it is ready.
fn shell_files(app: &tauri::AppHandle, payload: Value) {
    let mut q = SHELL_QUEUE.lock().unwrap();
    match q.as_mut() {
        Some(waiting) => waiting.push(payload),
        None => {
            let _ = tauri::Emitter::emit(app, "open-files", payload);
        }
    }
}

/// The interface listens now: what arrived before, in order (one entry per start).
#[tauri::command]
fn shell_ready() -> Vec<Value> {
    SHELL_QUEUE.lock().unwrap().take().unwrap_or_default()
}

/// Files given on the command line: "Open with", the right-click menu (`--tool <id> <file>`).
#[tauri::command]
async fn initial_files(app: tauri::AppHandle) -> Value {
    let args: Vec<String> = std::env::args().collect();
    let (tool, files) = shell::parse_args(&args);
    let infos = tauri::async_runtime::spawn_blocking(move || expand(&app, files)).await.unwrap_or_default();
    json!({ "tool": tool, "files": infos })
}

/// The Explorer menu on or off.
#[tauri::command]
fn shell_menu(enable: bool, lang: String, names: Option<HashMap<String, String>>) -> Result<usize, String> {
    let tools = registry::all();
    if enable {
        let names = names.unwrap_or_default();
        // Kept, so the installer can put the same menu back after an upgrade.
        state::set_internal("menuNames", &json!({ "lang": lang, "names": names }));
        shell::register(&tools, &lang, &names)
    } else {
        shell::unregister(&tools);
        Ok(0)
    }
}

/// Typed or pasted text, for the tools that take text (text to speech, Markdown, HTML):
/// saved as a file in Documents\\MQKit (the result goes next to it, where the person
/// can find both), named "<name> <date> <time>".
#[tauri::command]
async fn save_text(text: String, ext: String, name: String) -> Result<FileInfo, String> {
    if !matches!(ext.as_str(), "txt" | "md" | "html") || text.trim().is_empty() {
        return Err("invalid".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let dir = winutil::documents_dir().join("MQKit");
        std::fs::create_dir_all(&dir).map_err(|_| "permission_denied")?;
        let (date, time) = state::local_date_time();
        let stem = naming::safe(&format!("{} {date} {time}", name.trim()));
        let used = std::collections::HashSet::new();
        let mut path = dir.join(format!("{stem}.{ext}"));
        if path.exists() {
            path = naming::unique(&path, &used);
        }
        // Text with a BOM, so every program reads its letters right (ă, ü, 日本).
        let mut bytes = if ext == "txt" { vec![0xEF, 0xBB, 0xBF] } else { Vec::new() };
        bytes.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
        std::fs::write(&path, bytes).map_err(|_| "permission_denied")?;
        file_info(&path, None).ok_or_else(|| "permission_denied".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A recording made in the window (the microphone, through the WebView), saved like
/// typed text: in Documents\MQKit, then added as a file. The body is the bytes; the
/// headers give the kind (`x-ext`: webm, ogg or mp4) and the name (`x-name`, URI-encoded).
#[tauri::command]
async fn save_recording(request: tauri::ipc::Request<'_>) -> Result<FileInfo, String> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else { return Err("invalid".into()) };
    let header = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let ext = header("x-ext");
    if !matches!(ext.as_str(), "webm" | "ogg" | "mp4") || data.is_empty() || data.len() > 2_000_000_000 {
        return Err("invalid".into());
    }
    let name = percent_decode(&header("x-name"));
    let data = data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dir = winutil::documents_dir().join("MQKit");
        std::fs::create_dir_all(&dir).map_err(|_| "permission_denied")?;
        let (date, time) = state::local_date_time();
        let stem = naming::safe(&format!("{} {date} {time}", name.trim()));
        let mut path = dir.join(format!("{stem}.{ext}"));
        if path.exists() {
            path = naming::unique(&path, &std::collections::HashSet::new());
        }
        std::fs::write(&path, data).map_err(|_| "permission_denied")?;
        file_info(&path, None).ok_or_else(|| "permission_denied".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The voice recorder asks for the microphone from the app's own page, when the user
/// presses "Record…": allowed without WebView2's prompt (which names a localhost address
/// people don't know). Only the microphone, and only for the app's own page; Windows'
/// own microphone privacy switch still applies.
#[cfg(windows)]
fn allow_microphone(w: &tauri::WebviewWindow) {
    let _ = w.with_webview(|wv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        use webview2_com::PermissionRequestedEventHandler;
        let Ok(core) = wv.controller().CoreWebView2() else { return };
        let handler = PermissionRequestedEventHandler::create(Box::new(|_, args: Option<ICoreWebView2PermissionRequestedEventArgs>| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
            args.PermissionKind(&mut kind)?;
            let mut uri = windows::core::PWSTR::null();
            args.Uri(&mut uri)?;
            let uri = webview2_com::take_pwstr(uri);
            let own = ["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost", "http://localhost:5199"]
                .iter()
                .any(|o| uri == *o || uri.starts_with(&format!("{o}/")));
            if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE && own {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            }
            Ok(())
        }));
        let mut token = 0i64;
        let _ = core.add_PermissionRequested(&handler, &mut token);
    });
}

/// "%C8%98" → "Ș": the name sent in a header (headers carry only ASCII).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What to paste into a bug report: version, Windows, components, the end of the log.
/// No file names from the user's folder (see applog::redact), no settings beyond these.
#[tauri::command]
async fn diagnostics() -> String {
    tauri::async_runtime::spawn_blocking(|| {
        let engines: Vec<String> = json!(engines::list())
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|e| format!("{} {} {}", e["name"].as_str().unwrap_or(""), e["version"].as_str().unwrap_or(""), e["status"].as_str().unwrap_or("")))
            .collect();
        let env = env_now();
        format!(
            "MQKit {}\nWindows build {}\nLanguage {}\nOffice: Word {}, Excel {}, PowerPoint {}\nComponents:\n  {}\n\nLog (end):\n{}",
            env!("CARGO_PKG_VERSION"),
            env::os_build(),
            env["systemLanguage"].as_str().unwrap_or(""),
            env["office.word"].as_str().unwrap_or(""),
            env["office.excel"].as_str().unwrap_or(""),
            env["office.powerpoint"].as_str().unwrap_or(""),
            engines.join("\n  "),
            applog::redact(&applog::tail(60)),
        )
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
fn app_info() -> Value {
    json!({ "version": env!("CARGO_PKG_VERSION"), "dataDir": paths::data_dir(), "logsDir": paths::logs_dir() })
}

/// Development only: a file-based bridge so scripts can drive the UI
/// (`testdata/out/dev/in.js` → eval in the page → `dev_result` → `out.json`).
#[cfg(debug_assertions)]
fn dev_bridge(app: tauri::AppHandle) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("testdata").join("out").join("dev");
    let _ = std::fs::create_dir_all(&dir);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        let inp = dir.join("in.js");
        if let Ok(js) = std::fs::read_to_string(&inp) {
            let _ = std::fs::remove_file(&inp);
            if let Some(w) = app.get_webview_window("main") {
                let wrapped = format!(
                    "(async()=>{{let r;try{{r=await (async()=>{{{js}}})();}}catch(e){{r='EXCEPTION: '+e}};window.__TAURI_INTERNALS__.invoke('dev_result',{{value:JSON.stringify(r===undefined?null:r)}});}})()"
                );
                let _ = w.eval(&wrapped);
            }
        }
    });
}

#[cfg(debug_assertions)]
#[tauri::command]
fn dev_result(value: String) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("testdata").join("out").join("dev");
    let _ = std::fs::write(dir.join("out.json.tmp"), value);
    let _ = std::fs::rename(dir.join("out.json.tmp"), dir.join("out.json"));
}

#[cfg(not(debug_assertions))]
#[tauri::command]
fn dev_result(_value: String) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // DLLs only from Windows and the app's own folder, never from the folder the app was
    // started in (Explorer starts it in the selected file's folder).
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS};
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)) {
        let _ = std::env::set_current_dir(dir);
    }
    // The uninstaller removes the Explorer menu this way, without opening a window.
    if std::env::args().any(|a| a == "--unregister-shell") {
        shell::unregister(&[]);
        return;
    }
    // The installer puts it back after an upgrade (the previous version's uninstaller,
    // run first, took it off), as the settings ask, in their language.
    if std::env::args().any(|a| a == "--register-shell") {
        paths::init_cli();
        registry::load();
        let settings = state::get("settings");
        if settings["contextMenu"].as_bool() != Some(false) {
            let lang = settings["language"].as_str().filter(|l| *l != "system").map(str::to_string).unwrap_or_else(env::system_language);
            // The names the interface last gave, in that language; else the tools' own.
            let saved = state::get_internal("menuNames");
            let names: HashMap<String, String> = if saved["lang"] == json!(lang) { serde_json::from_value(saved["names"].clone()).unwrap_or_default() } else { HashMap::new() };
            let _ = shell::register(&registry::all(), &lang, &names);
        }
        return;
    }
    tauri::Builder::default()
        // A second start (one per file selected in Explorer) hands its files to this window.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let (tool, files) = shell::parse_args(&args);
            let handle = app.clone();
            std::thread::spawn(move || {
                let infos = expand(&handle, files);
                shell_files(&handle, json!({ "tool": tool, "files": infos }));
            });
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            paths::init(app.handle());
            for p in registry::load() {
                applog!("manifest: {p}");
            }
            // Work folders left by a crash, on the fixed drives; old previews; Office
            // programs a crashed run left open.
            std::thread::spawn(|| {
                paths::remove_tree(&paths::data_dir().join("tmp").join("preview"));
                paths::cleanup_stale(&paths::tmp_roots());
                winutil::kill_leftover_office();
            });
            if let Some(w) = app.get_webview_window("main") {
                // Windows 11: Mica as the base layer (title bar and navigation), Fluent's layering.
                // Elsewhere the UI paints solid colours.
                #[cfg(windows)]
                if env::os_build() >= 22000 {
                    use tauri::window::{Effect, EffectsBuilder};
                    if w.set_effects(EffectsBuilder::new().effect(Effect::Mica).build()).is_ok() {
                        env::MICA.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                #[cfg(windows)]
                allow_microphone(&w);
                let _ = w.show();
            }
            #[cfg(debug_assertions)]
            dev_bridge(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            save_text,
            save_recording,
            diagnostics,
            shell_ready,
            expand_paths,
            find_files,
            link_download,
            link_cancel,
            get_tools,
            get_env,
            state_get,
            state_set,
            analyze,
            estimate,
            preview_image,
            start_job,
            cancel_job,
            open_path,
            reveal,
            open_url,
            initial_files,
            app_info,
            engines_list,
            check_update,
            shell_menu,
            engine_download,
            engine_cancel,
            engine_remove,
            dev_result
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}

#[cfg(test)]
mod tls_tests {
    /// A server that leaves out its intermediate certificate (as some download mirrors do)
    /// is still reached. Needs the internet: `cargo test -p mqkit -- --ignored`.
    #[test]
    #[ignore]
    fn incomplete_chain_is_completed() {
        let agent: ureq::Agent = ureq::Agent::config_builder().tls_config(super::tls()).build().into();
        assert!(agent.get("https://incomplete-chain.badssl.com/").call().is_ok());
        let plain: ureq::Agent = ureq::Agent::config_builder().build().into();
        assert!(plain.get("https://incomplete-chain.badssl.com/").call().is_err(), "without it, the same request fails");
        assert!(agent.get("https://expired.badssl.com/").call().is_err(), "an expired certificate is still refused");
    }
}
