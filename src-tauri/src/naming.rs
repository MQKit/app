//! Final names (contract 8.3): template variables, safe names, conflicts.

use crate::jobs::OutputBlock;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn output_dir(o: &OutputBlock, input: &Path, tool_name: &str) -> PathBuf {
    let parent = input.parent().map(Path::to_path_buf).unwrap_or_default();
    match o.output_dir.as_str() {
        "subfolder" => {
            let name = if o.subfolder_name.trim().is_empty() { tool_name.to_string() } else { o.subfolder_name.clone() };
            let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let name = name.replace("{name}", &stem);
            parent.join(safe(&name))
        }
        "custom" => o.output_dir_path.clone().filter(|p| !p.as_os_str().is_empty()).unwrap_or(parent),
        _ => parent,
    }
}

/// Fills `{name}`, `{ext}`, `{n}`, `{page}`, `{total}`, `{pages}`, `{date}`, `{time}` and the plugin's variables.
/// With several results per input and no `{n}`/`{page}` in the template, `-{n}` is added
/// (not with `{file}`: a plugin variable that is already a distinct file name per result).
pub fn render(template: &str, input: &Path, vars: &Map<String, Value>, multi: bool) -> String {
    let mut t = if template.trim().is_empty() { "{name}".to_string() } else { template.to_string() };
    if multi && !t.contains("{n}") && !t.contains("{page}") && !t.contains("{file}") {
        // Outputs that bring their own name (a subtitle's language: film.eng, a font's face)
        // keep it rather than become film-1, film-2 (a template remembered before {file}).
        let own = vars.get("file").and_then(Value::as_str).is_some_and(|f| !f.is_empty());
        if own && t.contains("{name}") {
            t = t.replace("{name}", "{file}");
        } else {
            t.push_str("-{n}");
        }
    }
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = input.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let (date, time) = crate::state::local_date_time();
    let mut out = String::new();
    let mut rest = t.as_str();
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        let key = &rest[i + 1..i + j];
        let val = match key {
            "name" => stem.clone(),
            "ext" => ext.clone(),
            "date" => date.clone(),
            "time" => time.clone(),
            k => vars.get(k).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_default(),
        };
        out.push_str(&val);
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    let s = safe(&out);
    if s.is_empty() { stem } else { s }
}

/// Removes characters Windows does not allow in names, trailing dots and spaces, reserved names.
pub fn safe(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let upper = s.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    if ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3"].contains(&base) {
        s.insert(0, '_');
    }
    s.chars().take(200).collect()
}

/// How a path is compared with others in a job: Windows names ignore letter case, so
/// `Photo.JPG` and `photo.jpg` are the same file.
pub fn key(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().to_lowercase())
}

/// `name (1).ext`, `name (2).ext`… — the first one free on disk and not used in this job
/// (`used` holds `key`s).
pub fn unique(target: &Path, used: &HashSet<PathBuf>) -> PathBuf {
    let dir = target.parent().unwrap_or(Path::new("."));
    let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = target.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    for i in 1..10_000 {
        let p = dir.join(format!("{stem} ({i}){ext}"));
        if !p.exists() && !used.contains(&key(&p)) {
            return p;
        }
    }
    target.to_path_buf()
}

/// A free folder name next to `target`: "Name (1)", "Name (2)"… (a dot in a folder
/// name is not an extension).
pub fn unique_dir(target: &Path, used: &HashSet<PathBuf>) -> PathBuf {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    for i in 1..10_000 {
        let p = dir.join(format!("{name} ({i})"));
        if !p.exists() && !used.contains(&key(&p)) {
            return p;
        }
    }
    target.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn templates() {
        let v = json!({"page": 3}).as_object().unwrap().clone();
        assert_eq!(render("{name}-{page}", Path::new("C:\\a\\Contract ă.pdf"), &v, true), "Contract ă-3");
        assert_eq!(render("{name}_mic", Path::new("C:\\a\\x.jpg"), &Map::new(), false), "x_mic");
        let v = json!({"n": 2}).as_object().unwrap().clone();
        assert_eq!(render("{name}", Path::new("C:\\a\\x.heic"), &v, true), "x-2");
        assert_eq!(safe("a:b?. "), "a_b_");
        assert_eq!(safe("con"), "_con");
    }
}

#[cfg(test)]
mod own_names {
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn outputs_keep_the_name_they_bring() {
        let v = |x: serde_json::Value| x.as_object().unwrap().clone();
        let film = Path::new("C:/v/film.mkv");
        assert_eq!(super::render("{name}", film, &v(json!({ "file": "film.eng", "n": 1 })), true), "film.eng");
        assert_eq!(super::render("{name}_x", film, &v(json!({ "file": "film.fre", "n": 2 })), true), "film.fre_x");
        assert_eq!(super::render("{name}", film, &v(json!({ "n": 2 })), true), "film-2");
        assert_eq!(super::render("{name}", film, &v(json!({ "file": "film.eng" })), false), "film");
    }
}
