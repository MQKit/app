//! "From a link…": downloads a file the user pasted a link to into their Downloads
//! folder, as a browser would, so it is then a file like any other (results land next
//! to it). Nothing is sent but the request for that link. Share links of Google Drive,
//! Dropbox and GitHub are turned into their direct download.
//!
//! Errors are codes: `bad_link`, `network`, `http` (with the status), `page_not_file`,
//! `wrong_type` (with the extension), `too_big`, `disk_full`, `permission_denied`,
//! `cancelled`, `empty`.

use crate::FileInfo;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Larger than any file a tool would take; also bounds a server that never stops.
const MAX_BYTES: u64 = 64 << 30;

static ACTIVE: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = LazyLock::new(Default::default);

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub detail: String,
}

fn err(code: &'static str) -> Error {
    Error { code, detail: String::new() }
}

fn err_with(code: &'static str, detail: impl Into<String>) -> Error {
    Error { code, detail: detail.into() }
}

pub fn cancel(id: &str) {
    if let Some(f) = ACTIVE.lock().unwrap().get(id) {
        f.store(true, Ordering::Relaxed);
    }
}

/// Downloads `url` into the Downloads folder. `extensions` are what the tool takes
/// (empty = any file); `progress(done, total)` with total 0 when the server doesn't say.
pub fn download(id: &str, url: &str, extensions: &[String], progress: &dyn Fn(u64, u64)) -> Result<FileInfo, Error> {
    let flag = Arc::new(AtomicBool::new(false));
    ACTIVE.lock().unwrap().insert(id.to_string(), flag.clone());
    let res = fetch(url, extensions, &flag, progress);
    ACTIVE.lock().unwrap().remove(id);
    res
}

fn fetch(url: &str, extensions: &[String], cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<FileInfo, Error> {
    let url = direct(url.trim()).ok_or_else(|| err("bad_link"))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(6 * 3600)))
        .http_status_as_error(false)
        .max_redirects(10)
        .user_agent(concat!("MQKit/", env!("CARGO_PKG_VERSION"), " (Windows)"))
        .build()
        .into();
    let mut resp = agent.get(&url).call().map_err(|e| {
        eprintln!("link {url}: {e}");
        err("network")
    })?;
    let status = resp.status().as_u16();
    if status >= 400 {
        return Err(err_with("http", status.to_string()));
    }
    let headers = resp.headers();
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mime = header("content-type").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let total: u64 = header("content-length").parse().unwrap_or(0);
    let final_url = ureq::ResponseExt::get_uri(&resp).to_string();
    let name = choose_name(&header("content-disposition"), &final_url, &mime, extensions)?;
    if total > MAX_BYTES {
        return Err(err("too_big"));
    }

    let dir = downloads_dir();
    std::fs::create_dir_all(&dir).map_err(|_| err("permission_denied"))?;
    if total > 0 {
        if let Some(free) = crate::engines::free_space(&dir) {
            if free < total + (64 << 20) {
                return Err(err("disk_full"));
            }
        }
    }
    let dest = unique(&dir, &name);
    let part = dest.with_file_name(format!("{}.part", dest.file_name().unwrap().to_string_lossy()));
    let res = (|| {
        let mut file = std::fs::File::create(&part).map_err(|_| err("permission_denied"))?;
        let mut reader = resp.body_mut().with_config().limit(MAX_BYTES + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        let mut last = Instant::now();
        progress(0, total);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(err("cancelled"));
            }
            let n = reader.read(&mut buf).map_err(|e| {
                eprintln!("link {url} at {done}: {e}");
                if done > MAX_BYTES { err("too_big") } else { err("network") }
            })?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| if e.kind() == std::io::ErrorKind::StorageFull { err("disk_full") } else { err("permission_denied") })?;
            done += n as u64;
            if last.elapsed().as_millis() >= 150 {
                last = Instant::now();
                progress(done, total);
            }
        }
        if done == 0 {
            return Err(err("empty"));
        }
        if total > 0 && done < total {
            return Err(err("network"));
        }
        file.flush().map_err(|_| err("permission_denied"))?;
        drop(file);
        std::fs::rename(&part, &dest).map_err(|_| err("permission_denied"))?;
        progress(done, done);
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    crate::file_info(&dest, None).ok_or_else(|| err("permission_denied"))
}

/// The direct download for a link: only http(s); share pages of Google Drive,
/// Dropbox and GitHub become the file itself.
pub fn direct(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) || url.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = &url[url.find("://")? + 3..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..host_end].rsplit('@').next()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let after = &rest[host_end..];
    let path = after.split(['?', '#']).next().unwrap_or("");
    let query = after.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or("")).unwrap_or("");
    let param = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v.to_string()));

    if host == "drive.google.com" || host == "docs.google.com" {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let id = segs.iter().position(|s| *s == "d").and_then(|i| segs.get(i + 1)).map(|s| s.to_string()).or_else(|| param("id"));
        if let Some(id) = id.filter(|i| i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            // Docs, Sheets and Slides are exported in their Office format.
            let export = match segs.first() {
                Some(&"document") => Some(("document", "docx")),
                Some(&"spreadsheets") => Some(("spreadsheets", "xlsx")),
                Some(&"presentation") => Some(("presentation", "pptx")),
                _ => None,
            };
            return Some(match export {
                Some((kind, fmt)) if host == "docs.google.com" => format!("https://docs.google.com/{kind}/d/{id}/export?format={fmt}"),
                _ => format!("https://drive.usercontent.google.com/download?id={id}&export=download&confirm=t"),
            });
        }
    }
    if host == "dropbox.com" || host == "www.dropbox.com" {
        let kept: Vec<&str> = query.split('&').filter(|kv| !kv.is_empty() && !kv.starts_with("dl=") && !kv.starts_with("raw=")).collect();
        let q = kept.iter().copied().chain(["dl=1"]).collect::<Vec<_>>().join("&");
        return Some(format!("https://www.dropbox.com{path}?{q}"));
    }
    if host == "github.com" || host == "www.github.com" {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segs.len() > 4 && segs[2] == "blob" {
            return Some(format!("https://raw.githubusercontent.com/{}/{}/{}", segs[0], segs[1], segs[3..].join("/")));
        }
    }
    Some(url.to_string())
}

/// The file's name: the server's, else the link's last part, else "download"; with the
/// extension the content type says when the name has none the tool takes.
fn choose_name(disposition: &str, url: &str, mime: &str, extensions: &[String]) -> Result<String, Error> {
    let from_server = disposition_name(disposition);
    let from_url = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.split("://").nth(1))
        .and_then(|u| u.split_once('/').map(|(_, p)| p))
        .and_then(|p| p.rsplit('/').next())
        .map(percent_decode);
    let raw = from_server.or(from_url).unwrap_or_default();
    let mut name = sanitize(&raw);
    if name.is_empty() {
        name = "download".into();
    }
    let ext_of = |n: &str| Path::new(n).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let takes = |e: &str| extensions.is_empty() || extensions.iter().any(|x| x.eq_ignore_ascii_case(e));
    let ext = ext_of(&name);
    if !ext.is_empty() && takes(&ext) && !(mime == "text/html" && !matches!(ext.as_str(), "html" | "htm")) {
        return Ok(name);
    }
    if let Some(m) = mime_ext(mime) {
        if takes(m) {
            let known = !ext.is_empty() && mime_ext_known(&ext);
            let stem = if known { Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name.clone()) } else { name.clone() };
            return Ok(format!("{stem}.{m}"));
        }
    }
    if mime == "text/html" {
        return Err(err("page_not_file"));
    }
    if extensions.is_empty() {
        return Ok(name);
    }
    Err(err_with("wrong_type", if ext.is_empty() { mime_ext(mime).unwrap_or("").to_string() } else { ext }))
}

fn mime_ext_known(ext: &str) -> bool {
    MIME.iter().any(|(_, e)| *e == ext) || matches!(ext, "jpeg" | "htm" | "tiff")
}

const MIME: &[(&str, &str)] = &[
    ("application/pdf", "pdf"),
    ("image/jpeg", "jpg"),
    ("image/png", "png"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
    ("image/heic", "heic"),
    ("image/heif", "heif"),
    ("image/avif", "avif"),
    ("image/jxl", "jxl"),
    ("image/svg+xml", "svg"),
    ("image/tiff", "tif"),
    ("image/bmp", "bmp"),
    ("image/x-icon", "ico"),
    ("image/vnd.microsoft.icon", "ico"),
    ("image/vnd.adobe.photoshop", "psd"),
    ("video/mp4", "mp4"),
    ("video/webm", "webm"),
    ("video/quicktime", "mov"),
    ("video/x-matroska", "mkv"),
    ("video/x-msvideo", "avi"),
    ("audio/mpeg", "mp3"),
    ("audio/mp3", "mp3"),
    ("audio/wav", "wav"),
    ("audio/x-wav", "wav"),
    ("audio/wave", "wav"),
    ("audio/flac", "flac"),
    ("audio/x-flac", "flac"),
    ("audio/ogg", "ogg"),
    ("audio/opus", "opus"),
    ("audio/mp4", "m4a"),
    ("audio/x-m4a", "m4a"),
    ("audio/aac", "aac"),
    ("application/zip", "zip"),
    ("application/x-zip-compressed", "zip"),
    ("application/x-7z-compressed", "7z"),
    ("application/x-tar", "tar"),
    ("application/gzip", "gz"),
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", "docx"),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "xlsx"),
    ("application/vnd.openxmlformats-officedocument.presentationml.presentation", "pptx"),
    ("application/msword", "doc"),
    ("application/vnd.ms-excel", "xls"),
    ("application/vnd.ms-powerpoint", "ppt"),
    ("application/vnd.oasis.opendocument.text", "odt"),
    ("application/vnd.oasis.opendocument.spreadsheet", "ods"),
    ("application/vnd.oasis.opendocument.presentation", "odp"),
    ("application/rtf", "rtf"),
    ("text/rtf", "rtf"),
    ("text/csv", "csv"),
    ("text/markdown", "md"),
    ("text/plain", "txt"),
    ("text/html", "html"),
    ("application/json", "json"),
    ("application/xml", "xml"),
    ("text/xml", "xml"),
    ("application/epub+zip", "epub"),
    ("application/x-mobipocket-ebook", "mobi"),
    ("font/ttf", "ttf"),
    ("font/otf", "otf"),
    ("font/woff", "woff"),
    ("font/woff2", "woff2"),
    ("message/rfc822", "eml"),
    ("application/vnd.ms-outlook", "msg"),
    ("application/mbox", "mbox"),
    ("model/stl", "stl"),
    ("model/obj", "obj"),
    ("model/gltf-binary", "glb"),
    ("model/gltf+json", "gltf"),
    ("model/3mf", "3mf"),
    ("application/vnd.ms-package.3dmanufacturing-3dmodel+xml", "3mf"),
    ("image/vnd.dxf", "dxf"),
    ("text/vtt", "vtt"),
    ("application/x-subrip", "srt"),
];

fn mime_ext(mime: &str) -> Option<&'static str> {
    MIME.iter().find(|(m, _)| *m == mime).map(|(_, e)| *e)
}

/// `filename*=UTF-8''…` (preferred) or `filename="…"`.
fn disposition_name(h: &str) -> Option<String> {
    let mut plain = None;
    for part in h.split(';') {
        let part = part.trim();
        let Some((k, v)) = part.split_once('=') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        if k == "filename*" {
            let enc = v.splitn(3, '\'').nth(2).unwrap_or(v);
            let n = percent_decode(enc.trim_matches('"'));
            if !n.is_empty() {
                return Some(n);
            }
        } else if k == "filename" {
            plain = Some(v.trim_matches('"').to_string());
        }
    }
    plain.filter(|s| !s.is_empty())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A name Windows takes: no path, no reserved characters or names, not too long.
fn sanitize(s: &str) -> String {
    let base = s.rsplit(['/', '\\']).next().unwrap_or("");
    let mut n: String = base.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    n = n.trim().trim_end_matches(['.', ' ']).trim_start_matches('.').to_string();
    let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit()) {
        n.insert(0, '_');
    }
    if n.chars().count() > 150 {
        let ext = Path::new(&n).extension().map(|e| e.to_string_lossy().to_string()).filter(|e| e.len() <= 10);
        let keep: String = n.chars().take(140).collect();
        n = match ext {
            Some(e) => format!("{}.{e}", keep.trim_end_matches('.')),
            None => keep,
        };
    }
    n
}

/// `name`, else `name (2)`, `name (3)`… like a browser.
fn unique(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    let taken = |p: &Path| p.exists() || p.with_file_name(format!("{}.part", p.file_name().unwrap().to_string_lossy())).exists();
    if !taken(&first) {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..10000).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|c| !taken(c)).unwrap_or(first)
}

/// The user's Downloads folder (wherever they moved it).
#[cfg(windows)]
pub fn downloads_dir() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(std::env::temp_dir)
}

#[cfg(not(windows))]
pub fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_links_become_direct() {
        assert_eq!(direct("https://drive.google.com/file/d/1AbC_d-9/view?usp=sharing").unwrap(), "https://drive.usercontent.google.com/download?id=1AbC_d-9&export=download&confirm=t");
        assert_eq!(direct("https://drive.google.com/open?id=XyZ").unwrap(), "https://drive.usercontent.google.com/download?id=XyZ&export=download&confirm=t");
        assert_eq!(direct("https://docs.google.com/document/d/AbC/edit").unwrap(), "https://docs.google.com/document/d/AbC/export?format=docx");
        assert_eq!(direct("https://www.dropbox.com/scl/fi/abc/report.pdf?rlkey=k&dl=0").unwrap(), "https://www.dropbox.com/scl/fi/abc/report.pdf?rlkey=k&dl=1");
        assert_eq!(direct("https://github.com/u/r/blob/main/docs/a.md").unwrap(), "https://raw.githubusercontent.com/u/r/main/docs/a.md");
        assert_eq!(direct("https://example.com/a.pdf").unwrap(), "https://example.com/a.pdf");
        assert!(direct("file:///C:/x.pdf").is_none());
        assert!(direct("ftp://x/y").is_none());
        assert!(direct("https://").is_none());
    }

    #[test]
    fn names() {
        let pdf = vec!["pdf".to_string()];
        assert_eq!(choose_name("", "https://x.com/files/My%20Report.pdf?x=1", "application/pdf", &pdf).unwrap(), "My Report.pdf");
        assert_eq!(choose_name("attachment; filename=\"a.pdf\"; filename*=UTF-8''Rapor%C8%9B.pdf", "https://x.com/dl", "", &pdf).unwrap(), "Raporț.pdf");
        assert_eq!(choose_name("", "https://x.com/download", "application/pdf", &pdf).unwrap(), "download.pdf");
        assert_eq!(choose_name("", "https://x.com/doc.php", "application/pdf", &pdf).unwrap(), "doc.php.pdf");
        assert_eq!(choose_name("", "https://x.com/page.pdf", "text/html", &pdf).unwrap_err().code, "page_not_file");
        assert_eq!(choose_name("", "https://x.com/a.docx", "application/octet-stream", &pdf).unwrap_err().code, "wrong_type");
        assert_eq!(choose_name("", "https://x.com/", "application/octet-stream", &[]).unwrap(), "download");
        assert_eq!(choose_name("", "https://x.com/CON.pdf", "", &pdf).unwrap(), "_CON.pdf");
        assert_eq!(choose_name("filename=\"..\\..\\evil:.pdf\"", "https://x.com/", "", &pdf).unwrap(), "evil_.pdf");
    }

    #[test]
    fn unique_names() {
        let d = std::env::temp_dir().join(format!("convertor-link-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.pdf"), b"x").unwrap();
        std::fs::write(d.join("a (2).pdf"), b"x").unwrap();
        assert_eq!(unique(&d, "a.pdf").file_name().unwrap(), "a (3).pdf");
        assert_eq!(unique(&d, "b.pdf").file_name().unwrap(), "b.pdf");
        let _ = std::fs::remove_dir_all(&d);
    }
}
