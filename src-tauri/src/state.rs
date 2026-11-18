//! Small JSON stores in `%LOCALAPPDATA%\com.mqkit.app\state\`: settings, favorites,
//! last values per tool, history. No passwords are ever written here.

use serde_json::{json, Value};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

/// The stores the interface may read and write.
const ALLOWED: [&str; 4] = ["settings", "favorites", "lastValues", "history"];

pub fn get(name: &str) -> Value {
    if !ALLOWED.contains(&name) {
        return Value::Null;
    }
    let _g = LOCK.lock().unwrap();
    read(name)
}

pub fn set(name: &str, value: &Value) -> Result<(), String> {
    if !ALLOWED.contains(&name) {
        return Err("unknown store".into());
    }
    let _g = LOCK.lock().unwrap();
    write(name, value)
}

/// Read under the lock. A file that doesn't parse (cut short by a crash or a power cut)
/// falls back to the copy before the last write, so settings and history survive.
fn read(name: &str) -> Value {
    let dir = crate::paths::state_dir();
    let parse = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    parse(dir.join(format!("{name}.json"))).or_else(|| parse(dir.join(format!("{name}.json.bak")))).unwrap_or(Value::Null)
}

/// Write under the lock: to a temporary file flushed to disk, the previous version kept as
/// `.bak`, then renamed into place (retried: antivirus programs and the search indexer
/// hold freshly written files open for a moment).
fn write(name: &str, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let dir = crate::paths::state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!("{name}.json.tmp"));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let _ = std::fs::copy(&path, dir.join(format!("{name}.json.bak")));
    }
    let mut last = String::new();
    for _ in 0..10 {
        match std::fs::rename(&tmp, &path) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(last)
}

/// One more entry at the top of the history, read and written under one lock (two jobs
/// finishing together both get theirs).
pub fn add_history(entry: Value) {
    let _g = LOCK.lock().unwrap();
    let mut h = read("history");
    if !h.is_array() {
        h = json!([]);
    }
    let arr = h.as_array_mut().unwrap();
    arr.insert(0, entry);
    arr.truncate(200);
    let _ = write("history", &h);
}

/// Stores the app keeps for itself (not readable or writable by the interface).
pub fn get_internal(name: &str) -> Value {
    let _g = LOCK.lock().unwrap();
    read(name)
}

pub fn set_internal(name: &str, value: &Value) {
    let _g = LOCK.lock().unwrap();
    let _ = write(name, value);
}

/// Office programs (Word, Excel, PowerPoint) a worker started, with their start time, so
/// any left running after a crash can be stopped at the next start. Not visible to the
/// interface.
pub fn office_started(pid: u32, started: u64) {
    let _g = LOCK.lock().unwrap();
    let mut list = read_office();
    list.push(json!({ "pid": pid, "started": started }));
    let _ = write("office", &Value::Array(list));
}

pub fn office_gone(pids: &[u32]) {
    let _g = LOCK.lock().unwrap();
    let list: Vec<Value> = read_office().into_iter().filter(|e| !pids.contains(&(e["pid"].as_u64().unwrap_or(0) as u32))).collect();
    let _ = write("office", &Value::Array(list));
}

/// Those recorded and never seen to end; the record is cleared.
pub fn office_take_all() -> Vec<(u32, u64)> {
    let _g = LOCK.lock().unwrap();
    let list = read_office();
    let _ = write("office", &json!([]));
    list.iter().filter_map(|e| Some((e["pid"].as_u64()? as u32, e["started"].as_u64()?))).collect()
}

fn read_office() -> Vec<Value> {
    read("office").as_array().cloned().unwrap_or_default()
}

/// Local date and time for names: ("2026-11-16", "14-05").
pub fn local_date_time() -> (String, String) {
    let (y, mo, d, h, mi, _) = local_now();
    (format!("{y:04}-{mo:02}-{d:02}"), format!("{h:02}-{mi:02}"))
}

pub fn now_iso() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

#[cfg(windows)]
fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(not(windows))]
fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    (1970, 1, 1, 0, 0, 0)
}
