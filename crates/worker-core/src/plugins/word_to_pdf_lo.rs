//! Tool 5, second engine: Word → PDF with LibreOffice, for PCs without Microsoft Word.
//! The manifest is the one of `word-to-pdf` (office-worker); this is its `libreoffice` entry.

use super::{file_size, Analysis, Ctx, Plugin};
use contract::{codes, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct WordToPdfLo;

const INPUTS: &[&str] = &["doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "txt"];

fn soffice(req: &Request) -> Result<PathBuf, FileError> {
    if let Some(e) = req.engines.get("libreoffice") {
        for p in [e.path.join("program").join("soffice.bin"), e.path.join("soffice.bin")] {
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(FileError::with(codes::ENGINE_MISSING, json!({ "component": "libreoffice" })))
}

/// `file:///D:/a%20b/%C8%99` — LibreOffice wants the profile folder as a URL.
fn file_url(p: &Path) -> String {
    let s = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Cheap analysis without Office: format, password, macros, comments, page count from docProps.
/// A password typed for a protected document is checked here (Word's and LibreOffice's
/// analysis both come here), so a wrong one is said at once.
fn analyze_doc(path: &Path, password: Option<&str>) -> Analysis {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !INPUTS.contains(&ext.as_str()) {
        return Analysis::status("unsupported", codes::UNSUPPORTED_FILE);
    }
    let mut p = Map::new();
    p.insert("format".into(), json!(ext));
    p.insert("app".into(), json!("word"));
    let mut head = [0u8; 8];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    if n == 0 {
        return Analysis::status("corrupt", codes::CORRUPT_FILE);
    }
    let is_ole = head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]);
    let is_zip = head.starts_with(b"PK");
    if matches!(ext.as_str(), "docx" | "docm" | "dotx") {
        if is_ole {
            // An OOXML file wrapped in an OLE container is an encrypted one.
            p.insert("isEncrypted".into(), json!(true));
            match password.map(|pw| crate::common::offcrypto::unlock(path, pw)) {
                Some(Ok(Some(data))) => {
                    p.insert("needsPassword".into(), json!(false));
                    if !docx_props(&mut p, std::io::Cursor::new(data)) {
                        return Analysis::status("corrupt", codes::CORRUPT_FILE);
                    }
                    return Analysis::ok(p);
                }
                Some(Ok(None)) => {
                    p.insert("needsPassword".into(), json!(false));
                    return Analysis::ok(p);
                }
                Some(Err(code)) if code != codes::WRONG_PASSWORD => return Analysis::status("corrupt", code),
                wrong => {
                    p.insert("needsPassword".into(), json!(true));
                    let code = if wrong.is_some() { codes::WRONG_PASSWORD } else { codes::PASSWORD_REQUIRED };
                    return Analysis { status: "locked", props: p, warnings: vec![], code: Some(code.into()) };
                }
            }
        }
        if !is_zip {
            return Analysis::status("corrupt", codes::CORRUPT_FILE);
        }
        let Ok(file) = std::fs::File::open(path) else {
            return Analysis::status("corrupt", codes::CORRUPT_FILE);
        };
        if !docx_props(&mut p, file) {
            return Analysis::status("corrupt", codes::CORRUPT_FILE);
        }
    }
    // A Word 97-2003 document with a password: the FIB's fEncrypted flag. It can't be
    // checked here; Word checks the password when it converts.
    if ext == "doc" && is_ole && doc_encrypted(path) {
        p.insert("isEncrypted".into(), json!(true));
        if password.is_some() {
            p.insert("needsPassword".into(), json!(false));
            return Analysis::ok(p);
        }
        p.insert("needsPassword".into(), json!(true));
        return Analysis { status: "locked", props: p, warnings: vec![], code: Some(codes::PASSWORD_REQUIRED.into()) };
    }
    p.insert("needsPassword".into(), json!(false));
    Analysis::ok(p)
}

/// Macros, comments, tracked changes and the saved page count of a .docx; false when the
/// zip doesn't open or holds no document (a damaged file).
fn docx_props<R: std::io::Read + std::io::Seek>(p: &mut Map<String, Value>, reader: R) -> bool {
    let Ok(mut z) = zip::ZipArchive::new(reader) else { return false };
    let names: Vec<String> = z.file_names().filter_map(|n| n.ok().map(|n| n.to_string())).collect();
    if !names.iter().any(|n| n == "word/document.xml") {
        return false;
    }
    p.insert("hasMacros".into(), json!(names.iter().any(|n| n.ends_with("vbaProject.bin"))));
    p.insert("hasComments".into(), json!(names.iter().any(|n| n == "word/comments.xml")));
    if let Ok(mut doc) = z.by_name("word/document.xml") {
        let mut s = String::new();
        let _ = doc.read_to_string(&mut s);
        p.insert("hasTrackedChanges".into(), json!(s.contains("<w:ins ") || s.contains("<w:del ")));
    }
    if let Ok(mut app) = z.by_name("docProps/app.xml") {
        let mut s = String::new();
        let _ = app.read_to_string(&mut s);
        if let Some(n) = s.split("<Pages>").nth(1).and_then(|x| x.split("</Pages>").next()).and_then(|x| x.trim().parse::<u32>().ok()) {
            p.insert("cachedPageCount".into(), json!(n));
        }
    }
    true
}

fn doc_encrypted(path: &Path) -> bool {
    let Ok(mut c) = cfb::open(path) else { return false };
    let Ok(mut s) = c.open_stream("WordDocument") else { return false };
    let mut fib = [0u8; 12];
    s.read_exact(&mut fib).is_ok() && fib[11] & 0x01 != 0
}

impl Plugin for WordToPdfLo {
    fn id(&self) -> &'static str {
        "word-to-pdf-lo"
    }
    fn hidden(&self) -> bool {
        true
    }
    fn manifest(&self) -> Value {
        json!({})
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        analyze_doc(f.input_path(), req.secrets.files.get(&f.id).map(String::as_str))
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let input = f.input_path();
        let in_bytes = file_size(input);
        let dir = ctx.file_dir(&f.id)?;
        let mut opts = Map::new();
        let o = |t: &str, v: Value| json!({ "type": t, "value": v });
        if ctx.req.str_or("quality", "print") == "screen" {
            opts.insert("ReduceImageResolution".into(), o("boolean", json!("true")));
            opts.insert("MaxImageResolution".into(), o("long", json!("150")));
            opts.insert("Quality".into(), o("long", json!("80")));
        }
        if ctx.req.bool_or("pdfA", false) {
            opts.insert("SelectPdfVersion".into(), o("long", json!("2")));
        }
        if ctx.req.str_or("bookmarks", "headings") == "none" {
            opts.insert("ExportBookmarks".into(), o("boolean", json!("false")));
        }
        if ctx.req.str_or("markup", "clean") == "withComments" {
            opts.insert("ExportNotes".into(), o("boolean", json!("true")));
            opts.insert("ExportNotesInMargin".into(), o("boolean", json!("true")));
        }
        let filter = if opts.is_empty() { "pdf:writer_pdf_Export".to_string() } else { format!("pdf:writer_pdf_Export:{}", Value::Object(opts)) };
        let produced = convert_with_lo(ctx, f, &filter, &dir)?;
        let out = dir.join("result.pdf");
        std::fs::rename(&produced, &out).map_err(super::io_err)?;
        if !ctx.req.bool_or("pdfA", false) {
            compact(&out);
        }
        let pages = crate::common::pdf::open(&out, None).map(|d| d.get_pages().len()).unwrap_or(0);
        ctx.progress(&f.id, 1.0, "export", 2, 2);
        ctx.emit_output(&f.id, &out, "pdf", json!({ "pages": pages }), in_bytes);
        Ok(json!({ "engine": "libreoffice" }))
    }
}

/// LibreOffice writes many small objects one by one (a workbook: several per cell): saved
/// again in object streams the same PDF is often a quarter of the size. Kept only if smaller.
pub(super) fn compact(path: &Path) {
    let Ok(mut doc) = crate::common::pdf::open(path, None) else { return };
    let tmp = path.with_extension("compact.pdf");
    if crate::common::pdf::save(&mut doc, &tmp, false).is_ok()
        && std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(u64::MAX) < std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    {
        let _ = std::fs::rename(&tmp, path);
    }
    let _ = std::fs::remove_file(&tmp);
}

/// Runs LibreOffice's `--convert-to <filter>` on one file into `dir`, with progress and
/// cancel; returns the file it made. Shared by Word to PDF and Excel/PowerPoint to PDF.
pub(super) fn convert_with_lo(ctx: &Ctx, f: &FileRef, filter: &str, dir: &Path) -> Result<PathBuf, FileError> {
    convert_with_lo_as(ctx, f, None, filter, dir)
}

/// Like [`convert_with_lo`], with the import filter given (`(input, "CSV:59,34,76,1")`):
/// a prepared copy of the file, read the way the filter says.
pub(super) fn convert_with_lo_as(ctx: &Ctx, f: &FileRef, import: Option<(&Path, &str)>, filter: &str, dir: &Path) -> Result<PathBuf, FileError> {
    let exe = soffice(ctx.req)?;
    if let Some((input, infilter)) = import {
        return run_lo(ctx, f, &exe, input, Some(infilter), filter, dir);
    }
    let original = f.input_path();
    // A password-protected document: LibreOffice's command line takes no password, so it
    // converts a decrypted copy kept in the job's own folder, removed right after.
    let decrypted = decrypted_copy(ctx, f)?;
    let input = decrypted.as_deref().unwrap_or(original);
    let result = run_lo(ctx, f, &exe, input, None, filter, dir);
    if let Some(p) = &decrypted {
        let _ = std::fs::remove_file(p);
    }
    result
}

fn decrypted_copy(ctx: &Ctx, f: &FileRef) -> Result<Option<PathBuf>, FileError> {
    use crate::common::offcrypto;
    let path = f.input_path();
    if !offcrypto::is_encrypted(path) {
        // Word 97-2003 protection is not handled: say so rather than fail in LibreOffice.
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("doc")) && doc_encrypted(path) {
            return Err(FileError::new("password_old_format"));
        }
        return Ok(None);
    }
    let Some(password) = ctx.req.secrets.files.get(&f.id) else {
        return Err(FileError::new(codes::PASSWORD_REQUIRED));
    };
    let data = match offcrypto::decrypt(path, password) {
        Ok(d) => d,
        Err(offcrypto::Error::WrongPassword) => return Err(FileError::new(codes::WRONG_PASSWORD)),
        Err(offcrypto::Error::Corrupt) => return Err(FileError::new(codes::CORRUPT_FILE)),
        Err(offcrypto::Error::Unsupported) => return Err(FileError::new("password_old_format")),
    };
    let dir = ctx.work_dir.join(format!("{}-open", f.id));
    std::fs::create_dir_all(&dir).map_err(super::io_err)?;
    // Same name, so the result is named after the original.
    let copy = dir.join(path.file_name().unwrap_or_default());
    std::fs::write(&copy, data).map_err(super::io_err)?;
    Ok(Some(copy))
}

fn run_lo(ctx: &Ctx, f: &FileRef, exe: &Path, input: &Path, infilter: Option<&str>, filter: &str, dir: &Path) -> Result<PathBuf, FileError> {
    let profile = ctx.work_dir.join("lo-profile");

    ctx.progress(&f.id, 0.0, "open", 1, 2);
    // LibreOffice says nothing while it works: a slow climb, the heartbeat does the rest.
    let started = std::time::Instant::now();
    let mut attempt = 0;
    let (status, log) = loop {
        attempt += 1;
        let mut cmd = Command::new(exe);
        cmd.arg("--headless")
            .arg("--norestore")
            .arg("--nologo")
            .arg("--nodefault")
            .arg("--nolockcheck")
            .arg(format!("-env:UserInstallation={}", file_url(&profile)));
        if let Some(i) = infilter {
            cmd.arg(format!("--infilter={i}"));
        }
        cmd.arg("--convert-to")
            .arg(filter)
            .arg("--outdir")
            .arg(dir)
            .arg(input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let mut child = cmd.spawn().map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "libreoffice", "message": e.to_string() })))?;
        let mut err = child.stderr.take().unwrap();
        let err_thread = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let status = loop {
            if ctx.cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FileError::new(codes::CANCELLED));
            }
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    let t = started.elapsed().as_secs_f64();
                    ctx.progress(&f.id, 0.05 + 0.85 * (1.0 - (-t / 20.0).exp()), "export", 2, 2);
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => return Err(super::io_err(e)),
            }
        };
        let log = err_thread.join().unwrap_or_default();
        // 81: the first start created the profile and asks to be started again.
        if status.code() == Some(81) && attempt < 3 {
            continue;
        }
        break (status, log);
    };
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = filter.split(':').next().unwrap_or("pdf");
    let produced = dir.join(format!("{stem}.{ext}"));
    if !status.success() || !produced.is_file() {
        eprintln!("soffice: {log}");
        return Err(FileError::with(codes::ENGINE_FAILED, json!({ "exitCode": status.code(), "log": log.lines().rev().take(4).collect::<Vec<_>>() })));
    }
    Ok(produced)
}
