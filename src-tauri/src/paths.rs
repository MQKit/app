//! Where things live on disk (contract section 3).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::Manager;

pub const APP_DIR: &str = "Convertor";
pub const TMP_DIR: &str = ".convertor-tmp";

static RESOURCES: OnceLock<PathBuf> = OnceLock::new();
static DATA: OnceLock<PathBuf> = OnceLock::new();

pub fn init(app: &tauri::AppHandle) {
    let dir = app.path().resource_dir().unwrap_or_else(|_| std::env::current_exe().unwrap().parent().unwrap().to_path_buf());
    let _ = RESOURCES.set(dir);
    // Data lives in the app's own folder (%LOCALAPPDATA%\<identifier>), apart from the
    // installed program (%LOCALAPPDATA%\MQKit), so the uninstaller's "delete app data"
    // removes exactly it, and an uninstall never touches it otherwise.
    if let Ok(d) = app.path().app_local_data_dir() {
        migrate_old_data(&d);
        let _ = DATA.set(d);
    }
}

/// Where earlier builds kept their data, under %LOCALAPPDATA%: versions before 0.1.1 in
/// `Convertor`, then under the old app id until it became com.mqkit.app.
const OLD_DATA_DIRS: &[&str] = &["com.morfokit.convertor", APP_DIR];

/// Moves the data of earlier builds (downloaded engines, settings, history, logs) into
/// the app's folder, once, and removes the old folder.
fn migrate_old_data(new: &Path) {
    let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return };
    for dir in OLD_DATA_DIRS {
        let old = base.join(dir);
        if old != new && old.is_dir() {
            move_data(&old, new);
        }
    }
}

fn move_data(old: &Path, new: &Path) {
    let _ = std::fs::create_dir_all(new);
    for sub in ["engines", "state", "logs"] {
        let (from, to) = (old.join(sub), new.join(sub));
        if !from.is_dir() {
            continue;
        }
        if !to.exists() {
            let _ = std::fs::rename(&from, &to);
            continue;
        }
        // Both exist: move what the new one lacks (an engine, a state file).
        if let Ok(rd) = std::fs::read_dir(&from) {
            for e in rd.flatten() {
                let target = to.join(e.file_name());
                if !target.exists() {
                    let _ = std::fs::rename(e.path(), target);
                }
            }
        }
        let _ = std::fs::remove_dir(&from);
    }
    // The window's own cache (the app keeps nothing there) and leftover work files.
    remove_tree(&old.join("tmp"));
    remove_tree(&old.join("EBWebView"));
    // Gone only once empty: anything that couldn't be moved stays where it was.
    let _ = std::fs::remove_dir(old);
}

/// Installed, read-only: `workers\`, `plugins\`.
pub fn resources() -> &'static Path {
    RESOURCES.get().expect("paths::init")
}

pub fn plugins_dir() -> PathBuf {
    resources().join("plugins")
}

pub fn worker_exe(name: &str) -> PathBuf {
    resources().join("workers").join(format!("{name}.exe"))
}

/// The app's data (engines, state, logs) — survives updates.
pub fn data_dir() -> PathBuf {
    if let Some(d) = DATA.get() {
        return d.clone();
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(APP_DIR)
}

pub fn state_dir() -> PathBuf {
    let d = data_dir().join("state");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn logs_dir() -> PathBuf {
    let d = data_dir().join("logs");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn engines_dir() -> PathBuf {
    data_dir().join("engines")
}

/// The job work folder on the destination drive: `<root>\.convertor-tmp\<jobId>\`,
/// falling back to a hidden folder next to the destination, then to `%LOCALAPPDATA%`.
pub fn job_dir(dest: &Path, job_id: &str) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(root) = dest.ancestors().last() {
        candidates.push(root.join(TMP_DIR));
    }
    candidates.push(dest.join(TMP_DIR));
    candidates.push(data_dir().join("tmp"));
    for c in candidates {
        let d = c.join(job_id);
        if std::fs::create_dir_all(&d).is_ok() {
            hide(&c);
            return d;
        }
    }
    std::env::temp_dir().join(APP_DIR).join(job_id)
}

#[cfg(windows)]
fn hide(p: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    let w: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let _ = SetFileAttributesW(windows::core::PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_HIDDEN);
    }
}

#[cfg(not(windows))]
fn hide(_p: &Path) {}

/// Deletes a work folder without following junctions or symlinks.
pub fn remove_tree(p: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    if meta.file_type().is_symlink() {
        let _ = std::fs::remove_dir(p).or_else(|_| std::fs::remove_file(p));
        return;
    }
    if meta.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                remove_tree(&e.path());
            }
        }
        let _ = std::fs::remove_dir(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// Removes work folders left by a crash (called at startup).
pub fn cleanup_stale(roots: &[PathBuf]) {
    for r in roots {
        let t = r.join(TMP_DIR);
        if let Ok(rd) = std::fs::read_dir(&t) {
            for e in rd.flatten() {
                remove_tree(&e.path());
            }
        }
        let _ = std::fs::remove_dir(&t);
    }
}
