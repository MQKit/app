//! Where things live on disk (contract section 3).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::Manager;

pub const APP_DIR: &str = "Convertor";
/// The hidden work folder at the root of a destination drive (a result moved from there
/// to its folder is a rename, however large). Inside it, one folder per Windows user,
/// open to that user only.
pub const TMP_DIR: &str = ".mqkit-tmp";
/// The same folder of versions before 1.1 (shared by all users of the PC).
const OLD_TMP_DIR: &str = ".convertor-tmp";

/// When this run of the app started: work folders older than this are leftovers.
static STARTED: OnceLock<std::time::SystemTime> = OnceLock::new();

static RESOURCES: OnceLock<PathBuf> = OnceLock::new();
static DATA: OnceLock<PathBuf> = OnceLock::new();

pub fn init(app: &tauri::AppHandle) {
    let _ = STARTED.set(std::time::SystemTime::now());
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
        // Only a folder this app made: its state files or engines are there ("Convertor"
        // is a name another program could use too).
        let ours = old.join("state").join("settings.json").is_file() || old.join("state").join("history.json").is_file() || old.join("engines").join("ffmpeg").is_dir();
        if old != new && old.is_dir() && ours {
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

/// The app's identifier (tauri.conf.json): its data folder under %LOCALAPPDATA%.
pub const IDENTIFIER: &str = "com.mqkit.app";

/// For the installer's command-line calls, without a window: resources beside the
/// program, data where the app keeps it.
pub fn init_cli() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let _ = RESOURCES.set(dir);
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let _ = DATA.set(base.join(IDENTIFIER));
    }
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
    base.join(IDENTIFIER)
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

/// The job work folder on the destination drive: `<root>\.mqkit-tmp\<user>\<jobId>\`,
/// falling back to a hidden folder next to the destination, then to `%LOCALAPPDATA%`.
/// The user's folder is readable by that user only: a decrypted PDF or a converted
/// document waiting to be moved is nobody else's to read on a shared PC.
pub fn job_dir(dest: &Path, job_id: &str) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(root) = dest.ancestors().last() {
        candidates.push(root.join(TMP_DIR));
    }
    candidates.push(dest.join(TMP_DIR));
    for base in candidates {
        let mine = base.join(crate::winutil::user_sid());
        if std::fs::create_dir_all(&base).is_ok() && crate::winutil::create_private_dir(&mine) {
            hide(&base);
            let d = mine.join(job_id);
            if std::fs::create_dir_all(&d).is_ok() {
                return d;
            }
        }
    }
    let d = data_dir().join("tmp").join(job_id);
    if std::fs::create_dir_all(&d).is_ok() {
        return d;
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
/// Work folders left by a crash: this user's, from before this run started (a job begun
/// a moment ago is not touched); other users' only in the old shared folder, and only
/// after a day. Then the folders themselves, once empty.
pub fn cleanup_stale(roots: &[PathBuf]) {
    let started = STARTED.get().copied().unwrap_or_else(std::time::SystemTime::now);
    let day_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
    let older = |p: &Path, than: std::time::SystemTime| std::fs::symlink_metadata(p).and_then(|m| m.modified()).map(|t| t < than).unwrap_or(false);
    let clean = |dir: &Path, than: std::time::SystemTime| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if older(&e.path(), than) {
                    remove_tree(&e.path());
                }
            }
        }
        let _ = std::fs::remove_dir(dir);
    };
    for r in roots {
        let base = r.join(TMP_DIR);
        clean(&base.join(crate::winutil::user_sid()), started);
        let _ = std::fs::remove_dir(&base);
        clean(&r.join(OLD_TMP_DIR), day_ago);
    }
    clean(&data_dir().join("tmp"), started);
}

/// The roots to look at for leftovers: fixed local drives only (a disconnected network
/// drive can take seconds to answer).
pub fn tmp_roots() -> Vec<PathBuf> {
    ('C'..='Z')
        .map(|c| PathBuf::from(format!("{c}:\\")))
        .filter(|p| crate::winutil::has_recycle_bin(p))
        .filter(|p| p.join(TMP_DIR).exists() || p.join(OLD_TMP_DIR).exists())
        .collect()
}
