//! "From a folder…": every file under a folder that matches a pattern, for people who
//! want all the PDFs of a project, or every `**/*.md` of a documentation tree.
//!
//! Patterns (case does not matter, `;` between several):
//! - `*` any run of letters within a name, `?` one letter;
//! - `**` any number of folders (`docs/**/*.md`);
//! - `{pdf,docx}` one of the alternatives;
//! - a pattern without `/` looks at the file name only, in any folder.

use crate::FileInfo;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// More than this and the list is cut (the app would not be usable with more anyway).
const MAX_FILES: usize = 20_000;
const MAX_DEPTH: u32 = 64;
/// Files and folders looked at, at most (a whole drive or a build folder would take minutes).
const MAX_VISITED: usize = 400_000;

#[derive(Serialize)]
pub struct Found {
    pub files: Vec<FileInfo>,
    /// Files that matched (before the tool's own formats were checked).
    pub matched: usize,
    pub bytes: u64,
    pub truncated: bool,
    #[serde(skip)]
    visited: usize,
}

/// `{a,b}x` → `ax`, `bx` (nested braces too).
fn expand_braces(p: &str) -> Vec<String> {
    let Some(open) = p.find('{') else { return vec![p.to_string()] };
    let mut depth = 0;
    let mut close = None;
    for (i, c) in p[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else { return vec![p.to_string()] };
    let (head, inner, tail) = (&p[..open], &p[open + 1..close], &p[close + 1..]);
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts.iter().flat_map(|part| expand_braces(&format!("{head}{part}{tail}"))).collect()
}

/// One name (no `/`) against one pattern segment with `*` and `?`.
fn match_name(pat: &[char], name: &[char]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => match_name(&pat[1..], name) || (!name.is_empty() && match_name(pat, &name[1..])),
        (Some('?'), Some(_)) => match_name(&pat[1..], &name[1..]),
        (Some(p), Some(n)) => p == n && match_name(&pat[1..], &name[1..]),
        _ => false,
    }
}

/// Path segments against pattern segments, `**` standing for any number of them.
fn match_segments(pat: &[Vec<char>], path: &[Vec<char>]) -> bool {
    match pat.first() {
        None => path.is_empty(),
        Some(p) if p.len() == 2 && p[0] == '*' && p[1] == '*' => {
            (0..=path.len()).any(|skip| match_segments(&pat[1..], &path[skip..]))
        }
        Some(p) => !path.is_empty() && match_name(p, &path[0]) && match_segments(&pat[1..], &path[1..]),
    }
}

pub struct Pattern {
    alternatives: Vec<(bool, Vec<Vec<char>>)>,
}

impl Pattern {
    pub fn new(text: &str) -> Pattern {
        let mut alternatives = Vec::new();
        for raw in text.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()) {
            for p in expand_braces(&raw.to_lowercase().replace('\\', "/")) {
                let p = p.trim_start_matches("./").trim_start_matches('/').to_string();
                let whole_path = p.contains('/');
                let segs = p.split('/').filter(|s| !s.is_empty()).map(|s| s.chars().collect()).collect();
                alternatives.push((whole_path, segs));
            }
        }
        if alternatives.is_empty() {
            alternatives.push((false, vec![vec!['*']]));
        }
        Pattern { alternatives }
    }

    /// `rel` is the path below the chosen folder, with `/`.
    pub fn matches(&self, rel: &str) -> bool {
        let rel = rel.to_lowercase();
        let segs: Vec<Vec<char>> = rel.split('/').map(|s| s.chars().collect()).collect();
        self.alternatives.iter().any(|(whole, pat)| {
            if *whole {
                match_segments(pat, &segs)
            } else {
                segs.last().is_some_and(|name| pat.len() == 1 && match_name(&pat[0], name))
            }
        })
    }
}

pub fn find(root: &Path, pattern: &str, recursive: bool, hidden: bool, extensions: &[String]) -> Found {
    let pat = Pattern::new(pattern);
    let exts: Vec<String> = extensions.iter().map(|e| e.to_ascii_lowercase()).collect();
    let mut found = Found { files: Vec::new(), matched: 0, bytes: 0, truncated: false, visited: 0 };
    walk(root, root, 0, recursive, hidden, &pat, &exts, &mut found);
    found
}

#[allow(clippy::too_many_arguments)]
fn walk(dir: &Path, root: &Path, depth: u32, recursive: bool, hidden: bool, pat: &Pattern, exts: &[String], out: &mut Found) {
    if depth > MAX_DEPTH || out.truncated {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        out.visited += 1;
        if out.visited > MAX_VISITED {
            out.truncated = true;
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if !hidden && (name.starts_with('.') || is_hidden(&e.path())) {
            continue;
        }
        if name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db") {
            continue;
        }
        let path: PathBuf = e.path();
        if ft.is_dir() {
            if recursive {
                walk(&path, root, depth + 1, recursive, hidden, pat, exts, out);
            }
            continue;
        }
        let rel = path.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| name.clone());
        if !pat.matches(&rel) {
            continue;
        }
        out.matched += 1;
        let ext = path.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if !exts.is_empty() && !exts.contains(&ext) {
            continue;
        }
        if out.files.len() >= MAX_FILES {
            out.truncated = true;
            return;
        }
        if let Some(fi) = crate::file_info(&path, Some(root)) {
            out.bytes += fi.size;
            out.files.push(fi);
        }
    }
}

/// Windows' "hidden" or "system" attribute (AppData, $Recycle.Bin…).
fn is_hidden(p: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(m) = std::fs::symlink_metadata(p) {
            return m.file_attributes() & (0x2 | 0x4) != 0;
        }
    }
    let _ = p;
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let p = Pattern::new("*.pdf");
        assert!(p.matches("a.PDF") && p.matches("x/y/z.pdf") && !p.matches("a.pdfx"));
        let p = Pattern::new("docs/**/*.md");
        assert!(p.matches("docs/a.md") && p.matches("docs/x/y/b.md") && !p.matches("src/a.md"));
        let p = Pattern::new("*.{md,markdown}; raport-202?.docx");
        assert!(p.matches("n/readme.markdown") && p.matches("raport-2025.docx") && !p.matches("raport-25.docx"));
        let p = Pattern::new("**/drafts/*");
        assert!(p.matches("drafts/a.txt") && p.matches("x/drafts/b.pdf") && !p.matches("x/drafts/sub/c.pdf"));
        assert!(Pattern::new("").matches("anything.bin"));
        assert_eq!(expand_braces("a{b,c{d,e}}f"), vec!["abf", "acdf", "acef"]);
    }
}
