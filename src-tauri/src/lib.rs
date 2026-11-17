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
#[tauri::command]
fn expand_paths(app: tauri::AppHandle, paths: Vec<PathBuf>) -> Vec<FileInfo> {
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
fn get_env() -> Value {
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
            let msgs = worker::request(&t, &launch, "analyze", &req, timeout)?;
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
            .map(|f| results.remove(&f.id).unwrap_or_else(|| json!({ "file": f.id, "status": "failed", "code": "engine_failed", "props": {} })))
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
fn engines_list() -> Value {
    json!(engines::list())
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

#[tauri::command]
fn open_path(path: PathBuf) -> Result<(), String> {
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

/// Files given on the command line: "Open with", the right-click menu (`--tool <id> <file>`).
#[tauri::command]
fn initial_files(app: tauri::AppHandle) -> Value {
    let args: Vec<String> = std::env::args().collect();
    let (tool, files) = shell::parse_args(&args);
    json!({ "tool": tool, "files": expand_paths(app, files) })
}

/// The Explorer menu on or off.
#[tauri::command]
fn shell_menu(enable: bool, lang: String) -> Result<usize, String> {
    let tools = registry::all();
    if enable {
        shell::register(&tools, &lang)
    } else {
        shell::unregister(&tools);
        Ok(0)
    }
}

#[tauri::command]
fn app_info() -> Value {
    json!({ "version": env!("CARGO_PKG_VERSION"), "dataDir": paths::data_dir(), "logsDir": paths::logs_dir() })
}

/// Development only: a file-based bridge so scripts can drive the UI
/// (`testdata/out/dev/in.js` → eval in the page → `dev_result` → `out.json`).
#[cfg(debug_assertions)]
fn dev_bridge(app: tauri::AppHandle) {
    let dir = std::env::current_dir().unwrap_or_default().join("..").join("testdata").join("out").join("dev");
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
    let dir = std::env::current_dir().unwrap_or_default().join("..").join("testdata").join("out").join("dev");
    let _ = std::fs::write(dir.join("out.json.tmp"), value);
    let _ = std::fs::rename(dir.join("out.json.tmp"), dir.join("out.json"));
}

#[cfg(not(debug_assertions))]
#[tauri::command]
fn dev_result(_value: String) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The uninstaller removes the Explorer menu this way, without opening a window.
    if std::env::args().any(|a| a == "--unregister-shell") {
        shell::unregister(&[]);
        return;
    }
    tauri::Builder::default()
        // A second start (one per file selected in Explorer) hands its files to this window.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let (tool, files) = shell::parse_args(&args);
            let infos = expand_paths(app.clone(), files);
            let _ = tauri::Emitter::emit(app, "open-files", json!({ "tool": tool, "files": infos }));
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
                eprintln!("manifest: {p}");
            }
            // Work folders left by a crash, on the fixed drives; old previews.
            std::thread::spawn(|| {
                paths::remove_tree(&paths::data_dir().join("tmp").join("preview"));
                let roots: Vec<PathBuf> = ('C'..='Z').map(|c| PathBuf::from(format!("{c}:\\"))).filter(|p| p.join(paths::TMP_DIR).exists()).collect();
                paths::cleanup_stale(&roots);
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
                let _ = w.show();
            }
            #[cfg(debug_assertions)]
            dev_bridge(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
