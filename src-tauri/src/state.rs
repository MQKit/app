//! Small JSON stores in `%LOCALAPPDATA%\com.mqkit.app\state\`: settings, favorites,
//! last values per tool, history. No passwords are ever written here.

use serde_json::{json, Value};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

const ALLOWED: [&str; 4] = ["settings", "favorites", "lastValues", "history"];

pub fn get(name: &str) -> Value {
    if !ALLOWED.contains(&name) {
        return Value::Null;
    }
    let _g = LOCK.lock().unwrap();
    std::fs::read_to_string(crate::paths::state_dir().join(format!("{name}.json")))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

pub fn set(name: &str, value: &Value) -> Result<(), String> {
    if !ALLOWED.contains(&name) {
        return Err("unknown store".into());
    }
    let _g = LOCK.lock().unwrap();
    let path = crate::paths::state_dir().join(format!("{name}.json"));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn add_history(entry: Value) {
    let mut h = get("history");
    if !h.is_array() {
        h = json!([]);
    }
    let arr = h.as_array_mut().unwrap();
    arr.insert(0, entry);
    arr.truncate(200);
    let _ = set("history", &h);
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
