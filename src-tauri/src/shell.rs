//! The Explorer right-click menu: a "MQKit" submenu on each file type a tool
//! accepts, listing those tools. Per user (HKCU), no admin rights. On Windows 11
//! it shows under "Show more options" (the new menu needs a packaged app).
//!
//!   HKCU\Software\Classes\SystemFileAssociations\.pdf\shell\MQKit
//!       MUIVerb = "MQKit", Icon = app.exe, SubCommands = "", MultiSelectModel = Player
//!       shell\01pdf-compress   MUIVerb = "Comprimare PDF"
//!           command = "app.exe" --tool pdf-compress "%1"

use serde_json::Value;
use std::collections::BTreeMap;

const KEY: &str = "MQKit";
/// The menus of builds made before the app was named MQKit, taken off with ours.
const OLD_KEYS: &[&str] = &["Convertor", "MorfoKit"];

#[cfg(windows)]
mod reg {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn set(path: &str, name: Option<&str>, value: &str) -> bool {
        unsafe {
            let mut h = HKEY::default();
            let p = w(path);
            if RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut h, None).is_err() {
                return false;
            }
            let v = w(value);
            let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
            let n = name.map(w);
            let ok = RegSetValueExW(h, n.as_ref().map(|n| PCWSTR(n.as_ptr())).unwrap_or(PCWSTR::null()), None, REG_SZ, Some(bytes)).is_ok();
            let _ = RegCloseKey(h);
            ok
        }
    }

    pub fn delete_tree(path: &str) {
        unsafe {
            let p = w(path);
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()));
        }
    }
}

/// The tools' permanent numbers (ID:1, ID:2…), read from the app's own list so the menu
/// keeps the order the app shows.
const NUMBERS_TS: &str = include_str!("../../ui/src/lib/numbers.ts");

/// A tool's number from `numbers.ts` lines like `"pdf-compress": 1,` or `ocr: 15,`.
fn tool_number(id: &str) -> u32 {
    NUMBERS_TS
        .lines()
        .filter_map(|l| {
            let (key, rest) = l.trim().split_once(':')?;
            let key = key.trim().trim_matches('"');
            let n: u32 = rest.trim().trim_end_matches(',').trim().parse().ok()?;
            (key == id).then_some(n)
        })
        .next()
        .unwrap_or(u32::MAX)
}

/// Extensions → tools accepting them, by tool number.
fn by_extension(tools: &[Value]) -> BTreeMap<String, Vec<(String, Value)>> {
    let mut map: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    let mut sorted: Vec<&Value> = tools.iter().collect();
    sorted.sort_by_key(|t| {
        let id = t["id"].as_str().unwrap_or("");
        (tool_number(id), id.to_string())
    });
    for t in sorted {
        let id = t["id"].as_str().unwrap_or("").to_string();
        for e in t["input"]["extensions"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            map.entry(e.to_ascii_lowercase()).or_default().push((id.clone(), t["name"].clone()));
        }
    }
    map
}

/// Writes the menu for every file type (replacing what was there).
#[cfg(windows)]
pub fn register(tools: &[Value], lang: &str) -> Result<usize, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().to_string();
    unregister(tools);
    let map = by_extension(tools);
    for (ext, list) in &map {
        let base = format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{KEY}");
        reg::set(&base, Some("MUIVerb"), "MQKit");
        reg::set(&base, Some("Icon"), &format!("\"{exe}\",0"));
        reg::set(&base, Some("SubCommands"), "");
        reg::set(&base, Some("MultiSelectModel"), "Player");
        for (i, (id, name)) in list.iter().enumerate() {
            let label = name[lang].as_str().or(name["en"].as_str()).unwrap_or(id);
            let item = format!("{base}\\shell\\{:02}{id}", i + 1);
            reg::set(&item, Some("MUIVerb"), label);
            reg::set(&format!("{item}\\command"), None, &format!("\"{exe}\" --tool {id} \"%1\""));
        }
    }
    Ok(map.len())
}

/// Every extension a version of the app may have registered (used by the uninstaller).
const ALL_EXTS: &[&str] = &[
    "pdf", "jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff", "ico", "heic", "heif", "avif",
    "doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "txt",
    "mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v", "3gp", "mpg", "mpeg", "ts", "mts", "m2ts", "vob", "ogv",
    "wav", "m4a", "aac", "flac", "ogg", "opus", "wma", "amr", "mp3", "aiff",
];

#[cfg(windows)]
pub fn unregister(tools: &[Value]) {
    let mut exts: Vec<String> = by_extension(tools).into_keys().collect();
    exts.extend(ALL_EXTS.iter().map(|s| s.to_string()));
    exts.sort();
    exts.dedup();
    for ext in exts {
        reg::delete_tree(&format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{KEY}"));
        for old in OLD_KEYS {
            reg::delete_tree(&format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{old}"));
        }
    }
}

#[cfg(not(windows))]
pub fn register(_: &[Value], _: &str) -> Result<usize, String> {
    Ok(0)
}

#[cfg(not(windows))]
pub fn unregister(_: &[Value]) {}

/// `--tool <id> <files…>` from the command line (Explorer starts one process per file).
pub fn parse_args(args: &[String]) -> (Option<String>, Vec<std::path::PathBuf>) {
    let mut tool = None;
    let mut files = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--tool" && i + 1 < args.len() {
            tool = Some(args[i + 1].clone());
            i += 2;
            continue;
        }
        if !args[i].starts_with("--") {
            let p = std::path::PathBuf::from(&args[i]);
            if p.exists() {
                files.push(p);
            }
        }
        i += 1;
    }
    (tool, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn menu_follows_the_tool_numbers() {
        assert_eq!(tool_number("pdf-compress"), 1);
        assert_eq!(tool_number("ocr"), 15);
        assert_eq!(tool_number("meta-strip"), 43);
        assert_eq!(tool_number("no-such-tool"), u32::MAX);
        let tool = |id: &str| json!({ "id": id, "name": { "en": id }, "input": { "extensions": ["pdf"] } });
        let tools = vec![tool("meta-strip"), tool("ocr"), tool("pdf-merge"), tool("pdf-compress")];
        let order: Vec<String> = by_extension(&tools)["pdf"].iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(order, ["pdf-compress", "pdf-merge", "ocr", "meta-strip"]);
    }
}
