//! Printing HTML to PDF with Microsoft Edge, which every Windows 10 and 11 PC has.
//! Page size and margins come from an `@page` rule we add to the page.

use crate::plugins::Ctx;
use contract::{codes, FileError};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn exe() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(d) = std::env::var(var) {
            dirs.push(PathBuf::from(d).join("Microsoft").join("Edge").join("Application").join("msedge.exe"));
        }
    }
    dirs.into_iter().find(|p| p.exists())
}

/// The CSS that sets the paper, the margins, the orientation and background printing.
pub fn page_css(size: &str, landscape: bool, margin_mm: f64, backgrounds: bool) -> String {
    let size = match size {
        "letter" => "Letter",
        "a3" => "A3",
        "a5" => "A5",
        "legal" => "Legal",
        _ => "A4",
    };
    let orient = if landscape { " landscape" } else { "" };
    let bg = if backgrounds { "html, body, * { -webkit-print-color-adjust: exact !important; print-color-adjust: exact !important; }" } else { "" };
    format!("@page {{ size: {size}{orient}; margin: {margin_mm}mm; }} {bg}")
}

/// Prints `html` (a file) to `pdf`. Kills Edge on cancel or after `timeout`.
pub fn print(ctx: &Ctx, file: &str, html: &Path, pdf: &Path, timeout: Duration) -> Result<(), FileError> {
    print_with(ctx, file, html, pdf, timeout, false)
}

/// `untrusted`: a page we did not write (an e-mail): nothing is loaded from the internet
/// (a remote picture would tell the sender the mail was opened). Scripts are kept out by
/// the page's own Content-Security-Policy (Edge can't print with scripts switched off).
pub fn print_with(ctx: &Ctx, file: &str, html: &Path, pdf: &Path, timeout: Duration, untrusted: bool) -> Result<(), FileError> {
    let edge = exe().ok_or_else(|| FileError::with(codes::ENGINE_MISSING, json!({ "component": "msedge" })))?;
    let profile = ctx.work_dir.join(format!("edge-{file}"));
    std::fs::create_dir_all(&profile).map_err(crate::plugins::io_err)?;
    let url = file_url(html);
    let mut cmd = Command::new(edge);
    cmd.args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-extensions",
              "--no-pdf-header-footer", "--run-all-compositor-stages-before-draw", "--virtual-time-budget=10000",
              // The headings become the PDF's bookmarks.
              "--generate-pdf-document-outline"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--print-to-pdf={}", pdf.display()))
        .args(if untrusted { &["--host-resolver-rules=MAP * ~NOTFOUND"][..] } else { &[][..] })
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let mut child = cmd.spawn().map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "msedge", "message": e.to_string() })))?;
    let started = Instant::now();
    loop {
        if ctx.cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FileError::new(codes::CANCELLED));
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(FileError::new(codes::TIMEOUT));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(crate::plugins::io_err(e)),
        }
    }
    let _ = std::fs::remove_dir_all(&profile);
    if !pdf.exists() || std::fs::metadata(pdf).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err(FileError::with(codes::ENGINE_FAILED, json!({ "component": "msedge" })));
    }
    Ok(())
}

pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Adds `<base>` (so relative images and styles still load) and `css` to an HTML page.
pub fn inject(html: &str, base_dir: &Path, css: &str) -> String {
    let base = format!("<base href=\"{}/\">", file_url(base_dir));
    let style = format!("<style>{css}</style>");
    let lower = html.to_ascii_lowercase();
    if let Some(i) = lower.find("<head>") {
        let at = i + "<head>".len();
        format!("{}{base}{style}{}", &html[..at], &html[at..])
    } else if let Some(i) = lower.find("<html") {
        let at = html[i..].find('>').map(|j| i + j + 1).unwrap_or(i);
        format!("{}<head>{base}{style}</head>{}", &html[..at], &html[at..])
    } else {
        format!("<!doctype html><html><head><meta charset=\"utf-8\">{base}{style}</head><body>{html}</body></html>")
    }
}
