//! The app's own log, `logs\app.log` in its data folder: what went wrong with a download,
//! an install, a worker. The installed app has no console, so this is where it goes.
//! Kept under 1 MB (the previous part as `app.1.log`). Paths in the user's folder are
//! written as `%USERPROFILE%\…`, so a log shared for a bug report names no one.

use std::io::Write;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());
const MAX_BYTES: u64 = 1 << 20;

/// `applog!("download {url}: {err}")`: one timestamped line.
#[macro_export]
macro_rules! applog {
    ($($t:tt)*) => { $crate::applog::write(&format!($($t)*)) };
}

pub fn write(msg: &str) {
    #[cfg(debug_assertions)]
    eprintln!("{msg}");
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = crate::paths::logs_dir();
    let path = dir.join("app.log");
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("app.1.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {}", crate::state::now_iso(), redact(msg));
    }
}

/// The user's folder replaced by `%USERPROFILE%` (any letter case, either slash).
pub fn redact(text: &str) -> String {
    let Some(home) = std::env::var_os("USERPROFILE").map(|h| h.to_string_lossy().to_string()) else { return text.to_string() };
    if home.len() < 4 {
        return text.to_string();
    }
    let mut out = text.to_string();
    for form in [home.clone(), home.replace('\\', "/")] {
        let lower = out.to_lowercase();
        let needle = form.to_lowercase();
        let mut res = String::with_capacity(out.len());
        let mut last = 0;
        for (i, _) in lower.match_indices(&needle) {
            res.push_str(&out[last..i]);
            res.push_str("%USERPROFILE%");
            last = i + needle.len();
        }
        res.push_str(&out[last..]);
        out = res;
    }
    out
}

/// The end of the log, for "Copy details for a bug report".
pub fn tail(lines: usize) -> String {
    let text = std::fs::read_to_string(crate::paths::logs_dir().join("app.log")).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_folder_is_hidden() {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        if home.len() < 4 {
            return;
        }
        let line = format!("download {}\\Downloads\\a.pdf and {}/x", home.to_uppercase(), home.replace('\\', "/"));
        let r = super::redact(&line);
        assert!(!r.to_lowercase().contains(&home.to_lowercase()), "{r}");
        assert!(r.contains("%USERPROFILE%\\Downloads\\a.pdf"), "{r}");
    }
}
