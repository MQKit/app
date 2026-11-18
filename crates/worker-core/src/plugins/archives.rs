//! Archives: unpack (ZIP, 7z, RAR, TAR and its compressed forms, ISO, CAB, split
//! parts), pack files and folders into ZIP, 7z or TAR.GZ (with a password, split into
//! parts), and convert from one to another.
//!
//! ZIP, 7z and TAR (plain, gzip, bzip2, xz, zstd) are read here, so names in every
//! script come out right; RAR, ISO and CAB are read by the tar.exe in Windows
//! (libarchive), which can only write names in this PC's code page. Unpacking is guarded: entries cannot leave their
//! folder, links are not created, reserved Windows names are made safe, and it stops
//! before the disk fills up (archive bombs).

use super::{file_size, Analysis, Ctx, Plugin};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub struct Archives;

/// What the tool unpacks ("001" is the first part of a split archive).
const ARCHIVES: &[&str] = &["zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst", "tzst", "iso", "cab", "001"];
/// Room always left on the disk while unpacking.
const DISK_RESERVE: u64 = 512 * 1024 * 1024;

impl Plugin for Archives {
    fn id(&self) -> &'static str {
        "archives"
    }

    fn manifest(&self) -> Value {
        let packs = json!({ "field": "operation", "in": ["create", "convert"] });
        json!({
            "name": t("Arhive", "Archives"),
            "summary": t("ZIP, 7Z, RAR", "ZIP, 7Z, RAR"),
            "keywords": { "ro": ["zip", "rar", "7z", "arhivă", "dezarhivare", "dezarhivează", "arhivează"],
                          "en": ["zip", "rar", "7z", "archive", "unzip", "extract", "compress files"] },
            "category": "other",
            "icon": "file-zip",
            "input": { "extensions": ARCHIVES, "anyFile": true, "multiple": true, "folders": true, "mode": "combined" },
            "output": { "kind": "perInput", "extensions": ["folder", "zip", "7z", "tar.gz"],
                        "nameVariables": [ { "id": "file", "label": t("numele arhivei", "archive name") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["isArchive", "format", "needsPassword"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 3.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "operation", "type": "select", "label": t("Ce faci", "What to do"), "display": "chips",
                  "default": [ { "when": { "file": "isArchive", "eq": false, "quant": "any" }, "value": "create" }, { "value": "extract" } ],
                  "options": [ opt("extract", "Dezarhivează", "Unpack"), opt("create", "Arhivează împreună", "Pack together"),
                               opt("convert", "Schimbă formatul", "Change the format") ] },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "zip", "display": "chips", "showIf": packs,
                  "options": [ opt("zip", "ZIP", "ZIP"), opt("7z", "7Z (mai mic)", "7Z (smaller)"), opt("tar.gz", "TAR.GZ", "TAR.GZ") ] },
                { "key": "level", "type": "select", "label": t("Comprimare", "Compression"), "default": "normal", "showIf": packs,
                  "options": [ opt("store", "Fără (cel mai rapid)", "None (fastest)"), opt("normal", "Normală", "Normal"), opt("max", "Maximă (mai lent)", "Maximum (slower)") ] },
                { "key": "newPassword", "type": "password", "label": t("Parolă (opțional)", "Password (optional)"), "default": "",
                  "showIf": { "all": [ packs, { "field": "format", "in": ["zip", "7z"] } ] },
                  "help": t("Criptare AES-256. Fără parolă, oricine o poate deschide.", "AES-256 encryption. Without a password, anyone can open it.") },
                { "key": "hideNames", "type": "toggle", "default": true, "label": t("Ascunde și numele fișierelor", "Hide the file names too"),
                  "showIf": { "all": [ packs, { "field": "format", "eq": "7z" }, { "field": "newPassword", "ne": "" } ] } },
                { "key": "split", "type": "select", "advanced": true, "group": "more", "label": t("Împarte în bucăți", "Split into parts"), "default": 0,
                  "showIf": packs,
                  "options": [ opt(0, "Nu", "No"), opt(25, "25 MB (e-mail)", "25 MB (email)"), opt(100, "100 MB", "100 MB"),
                               opt(700, "700 MB (CD)", "700 MB (CD)"), opt(4000, "4 GB (stick FAT32)", "4 GB (FAT32 stick)") ],
                  "allowCustom": { "type": "number", "min": 1, "max": 100000, "unit": "MB" } }
            ],
            "rules": [
                { "id": "zip_aes", "severity": "info",
                  "when": { "all": [ packs, { "field": "format", "eq": "zip" }, { "field": "newPassword", "ne": "" } ] },
                  "message": t("Un ZIP cu parolă AES se deschide cu 7-Zip, WinRAR sau cu această aplicație; Exploratorul Windows nu-l poate deschide.",
                               "A ZIP with an AES password opens with 7-Zip, WinRAR or this app; Windows Explorer can't open it.") },
                { "id": "parts", "severity": "info", "when": { "all": [ packs, { "field": "split", "gt": 0 } ] },
                  "message": t("Bucățile (.001, .002…) se deschid cu 7-Zip sau cu această aplicație: adaugă prima bucată.",
                               "The parts (.001, .002…) open with 7-Zip or this app: add the first part.") },
                { "id": "not_archive", "severity": "warning",
                  "when": { "all": [ { "field": "operation", "in": ["extract", "convert"] }, { "file": "isArchive", "eq": false, "quant": "any" } ] },
                  "message": t("Unele fișiere nu sunt arhive; alege „Arhivează împreună” ca să le pui într-o arhivă.",
                               "Some files aren't archives; choose “Pack together” to put them in an archive.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{file}", "{file}") } },
            "errors": {
                "not_archive": t("Fișierul nu este o arhivă.", "The file isn't an archive."),
                "rar_encrypted": t("Arhivele RAR cu parolă se pot deschide doar cu WinRAR sau 7-Zip.", "RAR archives with a password open only with WinRAR or 7-Zip."),
                "unpack_failed": t("Arhiva este deteriorată sau într-un format pe care Windows nu-l poate citi.", "The archive is damaged or in a format Windows can't read."),
                "archive_bomb": t("Arhiva s-ar desface într-o mărime nefiresc de mare (o „bombă” de arhivă), așa că dezarhivarea a fost oprită.",
                                  "The archive would unpack to an unnaturally large size (an archive bomb), so unpacking was stopped."),
                "missing_part": t("Lipsește o bucată din arhivă (de exemplu .002). Pune toate bucățile în același dosar.", "A part of the archive is missing (for example .002). Put all the parts in the same folder."),
                "unsafe_entries": t("Am sărit peste intrări care încercau să scrie în afara dosarului.", "Skipped entries that tried to write outside the folder."),
                "links_skipped": t("Scurtăturile (legăturile) din arhivă nu au fost create.", "Shortcuts (links) inside the archive were not created."),
                "names_skipped": t("Unele fișiere au nume cu litere pe care dezarhivatorul din Windows nu le poate scrie și au fost sărite. Deschide arhiva cu 7-Zip pentru ele.",
                                   "Some files have names with letters the Windows unpacker can't write, so they were skipped. Open the archive with 7-Zip for those.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let path = f.input_path();
        let mut p = Map::new();
        let Some(format) = archive_format(path) else {
            p.insert("isArchive".into(), json!(false));
            return Analysis::ok(p);
        };
        p.insert("isArchive".into(), json!(true));
        p.insert("format".into(), json!(format));
        let locked = match format {
            "zip" => zip_encrypted(path),
            "7z" => sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).is_err_and(|e| is_password_error(&e)),
            _ => false,
        };
        p.insert("needsPassword".into(), json!(locked));
        if locked {
            return Analysis { status: "locked", props: p, warnings: vec![], code: Some(codes::PASSWORD_REQUIRED.into()) };
        }
        Analysis::ok(p)
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        match ctx.req.str_or("operation", "extract") {
            "create" => {
                let files = &ctx.req.files;
                ctx.run_one("*", || create(ctx, files));
            }
            op => {
                for f in &ctx.req.files {
                    if ctx.cancel.stop_after_current() {
                        break;
                    }
                    ctx.run_one(&f.id, || if op == "convert" { convert(ctx, f) } else { extract(ctx, f) });
                }
            }
        }
        Ok(())
    }
}

/// The archive format from the name ("tar.gz" counts as tar), or None.
fn archive_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let ext = name.rsplit('.').next()?;
    if ext == "001" {
        return Some("parts");
    }
    if [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tgz", ".tbz", ".tbz2", ".txz", ".tzst"].iter().any(|s| name.ends_with(s)) {
        return Some("tar");
    }
    ARCHIVES.iter().find(|a| **a == ext).copied()
}

/// The archive's name without its archive extensions ("photos.tar.gz" → "photos").
fn archive_stem(path: &Path) -> String {
    let mut name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for _ in 0..3 {
        let lower = name.to_ascii_lowercase();
        match lower.rsplit_once('.') {
            Some((_, ext)) if ARCHIVES.contains(&ext) || ext == "tar" => name.truncate(name.len() - ext.len() - 1),
            _ => break,
        }
    }
    if name.is_empty() { "archive".into() } else { name }
}

fn password_of(ctx: &Ctx, f: &FileRef) -> Option<String> {
    ctx.req.secrets.files.get(&f.id).cloned().filter(|p| !p.is_empty())
}

// ---------- unpacking ----------

/// Unpacks into a fresh folder named after the archive and hands the folder over.
fn extract(ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
    let path = f.input_path();
    let in_bytes = file_size(path);
    ctx.progress(&f.id, 0.0, "unpack", 1, 1);
    let dest = ctx.file_dir(&f.id)?.join("unpacked");
    let warnings = unpack(ctx, f, &dest)?;
    ctx.progress(&f.id, 0.9, "unpack", 1, 1);
    for w in warnings {
        ctx.out.warning(Some(&f.id), w, None);
    }
    ctx.progress(&f.id, 1.0, "unpack", 1, 1);
    let entries: Vec<_> = std::fs::read_dir(&dest).map(|rd| rd.flatten().collect()).unwrap_or_default();
    match entries.as_slice() {
        // A single file (a .gz, or an archive of one file): that file, next to the archive.
        [only] if only.file_type().map(|t| t.is_file()).unwrap_or(false) => {
            let p = only.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_string(), e.to_string()),
                _ => (name.clone(), String::new()),
            };
            if ext.is_empty() {
                ctx.emit_folder(&f.id, &dest, json!({ "file": archive_stem(&f.path) }), in_bytes);
            } else {
                ctx.emit_output(&f.id, &p, &ext, json!({ "file": stem }), in_bytes);
            }
        }
        _ => ctx.emit_folder(&f.id, &single_top_folder(&dest), json!({ "file": archive_stem(&f.path) }), in_bytes),
    }
    Ok(Value::Null)
}

/// When everything is inside one folder, that folder is the result (no "name\name\…").
fn single_top_folder(dest: &Path) -> PathBuf {
    let entries: Vec<_> = std::fs::read_dir(dest).map(|rd| rd.flatten().collect()).unwrap_or_default();
    match entries.as_slice() {
        [only] if only.file_type().map(|t| t.is_dir()).unwrap_or(false) => only.path(),
        _ => dest.to_path_buf(),
    }
}

/// Unpacks `f` into `dest`; returns warning codes.
fn unpack(ctx: &Ctx, f: &FileRef, dest: &Path) -> Result<Vec<&'static str>, FileError> {
    std::fs::create_dir_all(dest).map_err(super::io_err)?;
    std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
    let mut path = f.input_path().clone();
    let joined;
    let mut format = archive_format(&path).ok_or_else(|| FileError::new("not_archive"))?;
    if format == "parts" {
        joined = join_parts(ctx, f)?;
        path = joined.clone();
        format = archive_format(&path).ok_or_else(|| FileError::new("not_archive"))?;
    }
    if free_space(dest) < file_size(&path) + DISK_RESERVE {
        return Err(FileError::new(codes::DISK_FULL));
    }
    let password = password_of(ctx, f);
    let limit = unpack_limit(file_size(&path));
    let mut guard = Guard::new(dest, limit);
    let mut notes = Vec::new();
    let res = match format {
        "zip" => unzip(ctx, f, &path, dest, password.as_deref(), &mut guard),
        "7z" => un7z(ctx, f, &path, dest, password.as_deref(), &mut guard),
        "rar" | "iso" | "cab" => untar(ctx, f, &path, dest, limit).map(|(refused, unnamed)| {
            if refused {
                guard.unsafe_skipped += 1;
            }
            if unnamed {
                notes.push("names_skipped");
            }
        }),
        _ => read_tar(ctx, f, &path, &mut guard),
    };
    if format == "parts" || path != *f.input_path() {
        let _ = std::fs::remove_file(&path);
    }
    res?;
    if guard.unsafe_skipped > 0 {
        notes.push("unsafe_entries");
    }
    if guard.links_skipped > 0 {
        notes.push("links_skipped");
    }
    Ok(notes)
}

/// Split parts (name.7z.001, .002…) joined into one file in the work folder.
fn join_parts(ctx: &Ctx, f: &FileRef) -> Result<PathBuf, FileError> {
    let first = f.input_path();
    let name = first.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let base = name.strip_suffix(".001").or_else(|| name.strip_suffix(".001".to_uppercase().as_str())).unwrap_or(&name).to_string();
    let dir = first.parent().unwrap_or(Path::new("."));
    let joined = ctx.work_dir.join(format!("{}-{}", f.id, base));
    let mut out = std::io::BufWriter::new(std::fs::File::create(&joined).map_err(super::io_err)?);
    let mut n = 1;
    loop {
        let part = dir.join(format!("{base}.{n:03}"));
        if !part.exists() {
            break;
        }
        ctx.check_cancel()?;
        std::io::copy(&mut std::fs::File::open(&part).map_err(super::io_err)?, &mut out).map_err(super::io_err)?;
        n += 1;
    }
    out.flush().map_err(super::io_err)?;
    // A ZIP or 7z must end with its directory; a cut-off file means a missing part.
    if n == 2 && archive_format(Path::new(&base)).is_none() {
        return Err(FileError::new("not_archive"));
    }
    if archive_format(Path::new(&base)) == Some("zip") && zip::ZipArchive::new(std::fs::File::open(&joined).map_err(super::io_err)?).is_err() {
        return Err(FileError::new("missing_part"));
    }
    Ok(joined)
}

/// How much an archive may unpack to: a thousand times its size, at least 4 GB. Real
/// archives stay far below (text packs 5 to 20 times); a "zip bomb" of a few KB unpacks
/// to terabytes and would fill the disk.
fn unpack_limit(archive_bytes: u64) -> u64 {
    archive_bytes.saturating_mul(1000).max(4 << 30)
}

fn bomb(limit: u64) -> FileError {
    FileError::with("archive_bomb", json!({ "limitBytes": limit }))
}

/// Where unpacked entries may go, and how much room is left.
struct Guard {
    root: PathBuf,
    written: u64,
    next_check: u64,
    /// Stop (`archive_bomb`) past this many bytes written.
    limit: u64,
    unsafe_skipped: usize,
    links_skipped: usize,
    /// Places given so far, by their name as Windows compares it (case ignored).
    placed: std::collections::HashMap<String, PathBuf>,
}

impl Guard {
    fn new(root: &Path, limit: u64) -> Guard {
        Guard { root: root.to_path_buf(), written: 0, next_check: 0, limit, unsafe_skipped: 0, links_skipped: 0, placed: Default::default() }
    }

    /// Refuses at once an archive whose listed sizes are already past the limit.
    fn check_declared(&self, declared: u64) -> Result<(), FileError> {
        if declared > self.limit {
            return Err(bomb(self.limit));
        }
        Ok(())
    }

    /// The safe place for an entry name, or None (and counted) when it would leave the folder.
    fn place(&mut self, name: &str) -> Option<PathBuf> {
        match safe_relative(name) {
            Some(rel) if !rel.as_os_str().is_empty() => {
                // "Dup.txt" and "DUP.txt" are two files in a Linux or Mac archive but one name
                // on Windows: the second gets its own name instead of replacing the first.
                // The same name again (an updated copy in a TAR) still replaces it.
                let mut to = self.root.join(&rel);
                let mut n = 2;
                while let Some(prev) = self.placed.get(&to.to_string_lossy().to_lowercase()) {
                    if *prev == to {
                        break;
                    }
                    let stem = rel.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let ext = rel.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    to = self.root.join(rel.with_file_name(format!("{stem} ({n}){ext}")));
                    n += 1;
                }
                self.placed.insert(to.to_string_lossy().to_lowercase(), to.clone());
                Some(to)
            }
            Some(_) => None,
            None => {
                self.unsafe_skipped += 1;
                None
            }
        }
    }

    /// Copies an entry's data, stopping before the disk fills up.
    fn write(&mut self, src: &mut dyn Read, to: &Path) -> Result<(), FileError> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(super::io_err)?;
        }
        let mut out = std::fs::File::create(to).map_err(super::io_err)?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData || e.to_string().to_ascii_lowercase().contains("password") => {
                    return Err(FileError::new(codes::WRONG_PASSWORD));
                }
                Err(e) => return Err(FileError::with("unpack_failed", json!({ "message": e.to_string() }))),
            };
            out.write_all(&buf[..n]).map_err(super::io_err)?;
            self.written += n as u64;
            if self.written > self.limit {
                drop(out);
                let _ = std::fs::remove_file(to);
                return Err(bomb(self.limit));
            }
            if self.written >= self.next_check {
                if free_space(&self.root) < DISK_RESERVE {
                    return Err(FileError::new(codes::DISK_FULL));
                }
                self.next_check = self.written + 256 * 1024 * 1024;
            }
        }
        Ok(())
    }
}

/// An entry name as a path inside the folder: "..", drive letters and absolute paths
/// are refused (None); characters and names Windows forbids are replaced.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            _ => {}
        }
        let mut s: String = part.chars().map(|c| if "<>:\"|?*".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
        while s.ends_with('.') || s.ends_with(' ') {
            s.pop();
        }
        if s.is_empty() {
            s = "_".into();
        }
        let upper = s.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((upper.starts_with("COM") || upper.starts_with("LPT")) && upper.len() == 4 && upper.as_bytes()[3].is_ascii_digit());
        if reserved {
            s.insert(0, '_');
        }
        out.push(s);
    }
    debug_assert!(out.components().all(|c| matches!(c, Component::Normal(_))));
    Some(out)
}

fn free_space(dir: &Path) -> u64 {
    #[cfg(windows)]
    {
        let mut free = 0u64;
        let probe = if dir.exists() { dir } else { dir.parent().unwrap_or(dir) };
        let ok = unsafe { windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(&windows::core::HSTRING::from(probe.as_os_str()), Some(&mut free), None, None) };
        if ok.is_ok() {
            return free;
        }
    }
    let _ = dir;
    u64::MAX
}

fn zip_encrypted(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let Ok(mut z) = zip::ZipArchive::new(file) else { return false };
    (0..z.len()).any(|i| z.by_index_raw(i).map(|e| e.encrypted()).unwrap_or(false))
}

/// A ZIP entry name: UTF-8 when it is, otherwise this PC's OEM code page (what Windows
/// used to write it).
fn zip_name(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
        let n = unsafe { MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), raw, None) };
        if n > 0 {
            let mut buf = vec![0u16; n as usize];
            unsafe { MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), raw, Some(&mut buf)) };
            return String::from_utf16_lossy(&buf);
        }
    }
    String::from_utf8_lossy(raw).into_owned()
}

fn unzip(ctx: &Ctx, f: &FileRef, path: &Path, dest: &Path, password: Option<&str>, guard: &mut Guard) -> Result<(), FileError> {
    let file = std::fs::File::open(path).map_err(super::io_err)?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| FileError::new("unpack_failed"))?;
    let n = z.len();
    let declared = (0..n).filter_map(|i| z.by_index_raw(i).ok().map(|e| e.size())).fold(0u64, u64::saturating_add);
    guard.check_declared(declared)?;
    for i in 0..n {
        ctx.check_cancel()?;
        if i % 16 == 0 {
            ctx.progress(&f.id, 0.85 * i as f64 / n.max(1) as f64, "unpack", 1, 1);
        }
        let (name, is_dir, is_link, encrypted) = {
            let e = z.by_index_raw(i).map_err(|_| FileError::new("unpack_failed"))?;
            (zip_name(e.name_raw()), e.is_dir(), e.is_symlink(), e.encrypted())
        };
        if is_link {
            guard.links_skipped += 1;
            continue;
        }
        let Some(to) = guard.place(&name) else { continue };
        if is_dir {
            std::fs::create_dir_all(&to).map_err(super::io_err)?;
            continue;
        }
        let mut entry = match (encrypted, password) {
            (true, None) => return Err(FileError::new(codes::PASSWORD_REQUIRED)),
            (true, Some(p)) => z.by_index_decrypt(i, p.as_bytes()),
            (false, _) => z.by_index(i),
        }
        .map_err(|e| match e {
            zip::result::ZipError::InvalidPassword => FileError::new(codes::WRONG_PASSWORD),
            zip::result::ZipError::UnsupportedArchive(m) if m == zip::result::ZipError::PASSWORD_REQUIRED => FileError::new(codes::PASSWORD_REQUIRED),
            other => FileError::with("unpack_failed", json!({ "message": other.to_string() })),
        })?;
        guard.write(&mut entry, &to)?;
        if let Some(t) = entry.last_modified().and_then(zip_time_to_system) {
            if let Ok(fh) = std::fs::File::options().write(true).open(&to) {
                let _ = fh.set_modified(t);
            }
        }
    }
    let _ = dest;
    Ok(())
}

fn is_password_error(e: &sevenz_rust2::Error) -> bool {
    matches!(e, sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_))
}

fn un7z(ctx: &Ctx, f: &FileRef, path: &Path, dest: &Path, password: Option<&str>, guard: &mut Guard) -> Result<(), FileError> {
    let pw = password.map(sevenz_rust2::Password::from).unwrap_or_else(sevenz_rust2::Password::empty);
    let map_err = |e: sevenz_rust2::Error| match e {
        sevenz_rust2::Error::PasswordRequired => FileError::new(if password.is_some() { codes::WRONG_PASSWORD } else { codes::PASSWORD_REQUIRED }),
        sevenz_rust2::Error::MaybeBadPassword(_) => FileError::new(codes::WRONG_PASSWORD),
        other => FileError::with("unpack_failed", json!({ "message": other.to_string() })),
    };
    let mut reader = sevenz_rust2::ArchiveReader::open(path, pw).map_err(map_err)?;
    let total = reader.archive().files.len().max(1);
    guard.check_declared(reader.archive().files.iter().map(|e| e.size()).fold(0u64, u64::saturating_add))?;
    let mut done = 0usize;
    let mut failure: Option<FileError> = None;
    let res = reader.for_each_entries(|entry, data| {
        done += 1;
        if done.is_multiple_of(16) {
            ctx.progress(&f.id, 0.85 * done as f64 / total as f64, "unpack", 1, 1);
        }
        if ctx.check_cancel().is_err() {
            failure = Some(FileError::new(codes::CANCELLED));
            return Ok(false);
        }
        let Some(to) = guard.place(entry.name()) else { return Ok(true) };
        if entry.is_directory() {
            std::fs::create_dir_all(&to)?;
        } else if let Err(e) = guard.write(data, &to) {
            failure = Some(e);
            return Ok(false);
        }
        Ok(true)
    });
    if let Some(e) = failure {
        return Err(e);
    }
    res.map_err(map_err)?;
    let _ = dest;
    Ok(())
}

/// Everything else, with tar.exe (libarchive). By default it refuses "..", absolute
/// paths and writing through links; links it made are removed afterwards. It runs with
/// the disk watched, and is stopped if room runs out or on cancel.
fn untar(ctx: &Ctx, f: &FileRef, path: &Path, dest: &Path, limit: u64) -> Result<(bool, bool), FileError> {
    let tar = tar_exe().ok_or_else(|| FileError::with(codes::ENGINE_MISSING, json!({ "component": "tar" })))?;
    let log = ctx.work_dir.join(format!("{}-tar.log", f.id));
    let mut cmd = std::process::Command::new(tar);
    cmd.arg("-xf").arg(path).arg("-C").arg(dest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log).map_err(super::io_err)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "tar", "message": e.to_string() })))?;
    ctx.progress(&f.id, 0.3, "unpack", 1, 1);
    // tar.exe writes on its own: what it unpacked is read from the free space it used.
    let free_at_start = free_space(dest);
    let status = loop {
        if ctx.cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FileError::new(codes::CANCELLED));
        }
        let free = free_space(dest);
        if free < DISK_RESERVE {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FileError::new(codes::DISK_FULL));
        }
        if free_at_start != u64::MAX && free_at_start.saturating_sub(free) > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(bomb(limit));
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(150)),
            Err(e) => return Err(super::io_err(e)),
        }
    };
    remove_links(dest);
    let message = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    if !status.success() {
        let lower = message.to_ascii_lowercase();
        if lower.contains("encrypt") || lower.contains("passphrase") {
            return Err(FileError::new("rar_encrypted"));
        }
        // Some entries may have been refused while the rest unpacked fine.
        let any = std::fs::read_dir(dest).map(|mut rd| rd.next().is_some()).unwrap_or(false);
        if !any {
            return Err(FileError::with("unpack_failed", json!({ "message": message.lines().last().unwrap_or("") })));
        }
    }
    // Entries tar.exe refused: "..", absolute names, links pointing out; and names it
    // could not write (letters outside this PC's code page).
    let refused = message.contains("Path contains '..'") || message.contains("Hard-link target") || message.contains("symlink");
    Ok((refused, message.contains("unreadable filename")))
}

/// Counts what has been read from the archive file, for progress.
struct Counting<R> {
    inner: R,
    read: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

/// TAR, compressed or not, or a single compressed file (.gz, .bz2, .xz, .zst).
fn read_tar(ctx: &Ctx, f: &FileRef, path: &Path, guard: &mut Guard) -> Result<(), FileError> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let total = file_size(path).max(1);
    let read = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let file = Counting { inner: std::io::BufReader::new(std::fs::File::open(path).map_err(super::io_err)?), read: read.clone() };
    let bad = |e: std::io::Error| FileError::with("unpack_failed", json!({ "message": e.to_string() }));
    let ends = |s: &[&str]| s.iter().any(|x| name.ends_with(x));
    let mut data: Box<dyn Read> = if ends(&[".gz", ".tgz"]) {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if ends(&[".bz2", ".tbz", ".tbz2"]) {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else if ends(&[".xz", ".txz"]) {
        Box::new(lzma_rust2::XzReader::new(file, true))
    } else if ends(&[".zst", ".tzst"]) {
        Box::new(ruzstd::decoding::StreamingDecoder::new(file).map_err(|e| FileError::with("unpack_failed", json!({ "message": e.to_string() })))?)
    } else {
        Box::new(file)
    };
    // A TAR has "ustar" in its first block (or is named so); anything else is one file.
    let mut head = vec![0u8; 512];
    let mut got = 0;
    while got < 512 {
        match data.read(&mut head[got..]).map_err(bad)? {
            0 => break,
            n => got += n,
        }
    }
    head.truncate(got);
    let is_tar = (got == 512 && &head[257..262] == b"ustar") || name.contains(".tar") || ends(&[".tgz", ".tbz", ".tbz2", ".txz", ".tzst"]);
    let mut stream = std::io::Cursor::new(head).chain(data);
    if !is_tar {
        let inner = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let to = guard.place(&inner).unwrap_or_else(|| guard.root.join("file"));
        return guard.write(&mut stream, &to);
    }
    let mut archive = tar::Archive::new(stream);
    for entry in archive.entries().map_err(bad)? {
        ctx.check_cancel()?;
        ctx.progress(&f.id, (0.85 * read.get() as f64 / total as f64).min(0.85), "unpack", 1, 1);
        let mut entry = entry.map_err(bad)?;
        let kind = entry.header().entry_type();
        let entry_name = zip_name(&entry.path_bytes());
        if kind.is_symlink() || kind.is_hard_link() {
            guard.links_skipped += 1;
            continue;
        }
        if !(kind.is_file() || kind.is_dir() || kind.is_contiguous() || kind.is_gnu_sparse()) {
            continue;
        }
        let Some(to) = guard.place(&entry_name) else { continue };
        if kind.is_dir() {
            std::fs::create_dir_all(&to).map_err(super::io_err)?;
            continue;
        }
        let mtime = entry.header().mtime().ok();
        guard.write(&mut entry, &to)?;
        if let Some(t) = mtime.filter(|t| *t > 0) {
            if let Ok(fh) = std::fs::File::options().write(true).open(&to) {
                let _ = fh.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(t));
            }
        }
    }
    Ok(())
}


fn tar_exe() -> Option<PathBuf> {
    let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let p = sys.join("System32").join("tar.exe");
    p.exists().then_some(p)
}

/// Symbolic links and junctions inside an unpacked folder are deleted (not followed).
fn remove_links(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            meta.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if meta.file_type().is_symlink() || reparse {
            let _ = std::fs::remove_file(e.path()).or_else(|_| std::fs::remove_dir(e.path()));
        } else if meta.is_dir() {
            remove_links(&e.path());
        }
    }
}

fn zip_time_to_system(t: zip::DateTime) -> Option<std::time::SystemTime> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
        let local = SYSTEMTIME { wYear: t.year(), wMonth: t.month() as u16, wDay: t.day() as u16, wHour: t.hour() as u16, wMinute: t.minute() as u16, wSecond: t.second() as u16, ..Default::default() };
        let mut utc = SYSTEMTIME::default();
        let mut ft = FILETIME::default();
        unsafe {
            TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).ok()?;
            SystemTimeToFileTime(&utc, &mut ft).ok()?;
        }
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        // FILETIME counts 100 ns from 1601; UNIX time starts 11 644 473 600 s later.
        let secs = (ticks / 10_000_000).checked_sub(11_644_473_600)?;
        return Some(std::time::UNIX_EPOCH + Duration::from_secs(secs));
    }
    #[allow(unreachable_code)]
    {
        let _ = t;
        None
    }
}

fn system_to_zip_time(t: std::time::SystemTime) -> Option<zip::DateTime> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() + 11_644_473_600;
        let ticks = secs * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
        unsafe {
            FileTimeToSystemTime(&ft, &mut utc).ok()?;
            SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
        }
        return zip::DateTime::from_date_and_time(local.wYear, local.wMonth as u8, local.wDay as u8, local.wHour as u8, local.wMinute as u8, local.wSecond as u8).ok();
    }
    #[allow(unreachable_code)]
    {
        let _ = t;
        None
    }
}

// ---------- packing ----------

/// One file going into an archive: where it is and its name inside.
struct Source {
    path: PathBuf,
    name: String,
}

/// The files as named inside the archive: a dropped folder keeps its structure (the
/// folder itself on top, as Explorer does); loose files go at the root. Clashing names
/// get " (2)".
fn sources(files: &[FileRef]) -> Vec<Source> {
    let mut used = std::collections::HashSet::new();
    let mut out = Vec::new();
    for f in files {
        let rel = f
            .base
            .as_ref()
            .and_then(|b| f.path.strip_prefix(b.parent().unwrap_or(b)).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(f.path.file_name().unwrap_or_default()));
        let mut name = rel.to_string_lossy().replace('\\', "/");
        if !used.insert(name.to_lowercase()) {
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() && !e.contains('/') => (s.to_string(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            let mut i = 2;
            while !used.insert(format!("{stem} ({i}){ext}").to_lowercase()) {
                i += 1;
            }
            name = format!("{stem} ({i}){ext}");
        }
        out.push(Source { path: f.path.clone(), name });
    }
    out
}

/// A name for a new archive: the dropped folder, the only file, or their folder.
fn pack_name(files: &[FileRef]) -> String {
    let bases: std::collections::HashSet<_> = files.iter().map(|f| f.base.clone()).collect();
    if let (1, Some(Some(b))) = (bases.len(), bases.iter().next()) {
        if let Some(n) = b.file_name() {
            return n.to_string_lossy().to_string();
        }
    }
    if let [only] = files {
        return only.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "archive".into());
    }
    files
        .first()
        .and_then(|f| f.path.parent())
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into())
}

fn create(ctx: &Ctx, files: &[FileRef]) -> Result<Value, FileError> {
    ctx.progress("*", 0.0, "pack", 1, 1);
    let srcs = sources(files);
    let in_bytes: u64 = srcs.iter().map(|s| file_size(&s.path)).sum();
    let name = pack_name(files);
    pack_and_emit(ctx, "*", &srcs, &name, in_bytes)
}

/// Unpacks into the work folder, then packs that into the chosen format.
fn convert(ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
    let in_bytes = file_size(f.input_path());
    ctx.progress(&f.id, 0.0, "unpack", 1, 2);
    let dir = ctx.work_dir.join(format!("{}-convert", f.id));
    let _ = std::fs::remove_dir_all(&dir);
    unpack(ctx, f, &dir)?;
    let mut srcs = Vec::new();
    collect(&dir, &dir, &mut srcs);
    if srcs.is_empty() {
        return Err(FileError::new("unpack_failed"));
    }
    let res = pack_and_emit(ctx, &f.id, &srcs, &archive_stem(&f.path), in_bytes);
    let _ = std::fs::remove_dir_all(&dir);
    res
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<Source>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => collect(&p, root, out),
            Ok(t) if t.is_file() => {
                let name = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                out.push(Source { path: p, name });
            }
            _ => {}
        }
    }
}

fn pack_and_emit(ctx: &Ctx, unit: &str, srcs: &[Source], name: &str, in_bytes: u64) -> Result<Value, FileError> {
    let req = ctx.req;
    let format = req.str_or("format", "zip");
    let ext = match format {
        "7z" => "7z",
        "tar.gz" => "tar.gz",
        _ => "zip",
    };
    let level = req.str_or("level", "normal");
    let password = req.str_or("newPassword", "").to_string();
    let dir = ctx.file_dir(unit)?;
    let out = dir.join(format!("archive.{ext}"));
    let total = in_bytes.max(1);
    let mut done = 0u64;
    let mut tick = |ctx: &Ctx, bytes: u64| -> Result<(), FileError> {
        done += bytes;
        ctx.progress(unit, (done as f64 / total as f64).min(0.95), "pack", 1, 1);
        ctx.check_cancel()
    };
    match ext {
        "zip" => write_zip(ctx, srcs, &out, level, &password, &mut tick)?,
        "7z" => write_7z(ctx, srcs, &out, level, &password, req.bool_or("hideNames", true), &mut tick)?,
        _ => write_targz(ctx, srcs, &out, level, &mut tick)?,
    }
    let split_mb = req.f64_or("split", 0.0);
    let size = file_size(&out);
    if split_mb > 0.0 && size > (split_mb * 1024.0 * 1024.0) as u64 {
        let part = (split_mb * 1024.0 * 1024.0) as u64;
        let mut src = std::fs::File::open(&out).map_err(super::io_err)?;
        let mut i = 1;
        loop {
            let p = dir.join(format!("archive.{ext}.{i:03}"));
            let mut w = std::fs::File::create(&p).map_err(super::io_err)?;
            let copied = std::io::copy(&mut (&mut src).take(part), &mut w).map_err(super::io_err)?;
            drop(w);
            if copied == 0 {
                let _ = std::fs::remove_file(&p);
                break;
            }
            ctx.emit_output(unit, &p, &format!("{ext}.{i:03}"), json!({ "file": name }), if i == 1 { in_bytes } else { 0 });
            i += 1;
        }
        let _ = std::fs::remove_file(&out);
    } else {
        ctx.emit_output(unit, &out, ext, json!({ "file": name }), in_bytes);
    }
    ctx.progress(unit, 1.0, "pack", 1, 1);
    Ok(Value::Null)
}

type Tick<'a> = dyn FnMut(&Ctx, u64) -> Result<(), FileError> + 'a;

fn write_zip(ctx: &Ctx, srcs: &[Source], out: &Path, level: &str, password: &str, tick: &mut Tick) -> Result<(), FileError> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(out).map_err(super::io_err)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let bad = |e: zip::result::ZipError| FileError::with(codes::ENGINE_FAILED, json!({ "message": e.to_string() }));
    let mut dirs = std::collections::HashSet::new();
    let mut dir_opts = SimpleFileOptions::default();
    if let Some(t) = system_to_zip_time(std::time::SystemTime::now()) {
        dir_opts = dir_opts.last_modified_time(t);
    }
    for s in srcs {
        // Folders as entries too, so empty-looking trees open the same everywhere.
        let mut acc = String::new();
        for part in s.name.split('/').collect::<Vec<_>>().iter().rev().skip(1).rev() {
            acc.push_str(part);
            acc.push('/');
            if dirs.insert(acc.clone()) {
                z.add_directory(acc.clone(), dir_opts).map_err(bad)?;
            }
        }
        let meta = std::fs::metadata(&s.path).map_err(super::io_err)?;
        let (method, lvl) = match level {
            "store" => (zip::CompressionMethod::Stored, None),
            "max" => (zip::CompressionMethod::Deflated, Some(9)),
            _ => (zip::CompressionMethod::Deflated, Some(6)),
        };
        let mut o = SimpleFileOptions::default().compression_method(method).compression_level(lvl).large_file(meta.len() >= 0xFFFF_FFFF);
        if let Some(t) = meta.modified().ok().and_then(system_to_zip_time) {
            o = o.last_modified_time(t);
        }
        let mut src = std::fs::File::open(&s.path).map_err(super::io_err)?;
        if password.is_empty() {
            z.start_file(&s.name, o).map_err(bad)?;
        } else {
            z.start_file(&s.name, o.with_aes_encryption(zip::AesMode::Aes256, password)).map_err(bad)?;
        }
        copy_counting(ctx, &mut src, &mut z, tick)?;
    }
    z.finish().map_err(bad)?.flush().map_err(super::io_err)
}

fn copy_counting(ctx: &Ctx, src: &mut dyn Read, dst: &mut dyn Write, tick: &mut Tick) -> Result<(), FileError> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = src.read(&mut buf).map_err(super::io_err)?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n]).map_err(super::io_err)?;
        tick(ctx, n as u64)?;
    }
}

fn write_7z(ctx: &Ctx, srcs: &[Source], out: &Path, level: &str, password: &str, hide_names: bool, tick: &mut Tick) -> Result<(), FileError> {
    use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
    let bad = |e: sevenz_rust2::Error| FileError::with(codes::ENGINE_FAILED, json!({ "message": e.to_string() }));
    let file = std::fs::File::create(out).map_err(super::io_err)?;
    let mut w = sevenz_rust2::ArchiveWriter::new(file).map_err(bad)?;
    let mut methods: Vec<sevenz_rust2::EncoderConfiguration> = Vec::new();
    if !password.is_empty() {
        methods.push(AesEncoderOptions::new(sevenz_rust2::Password::from(password)).into());
    }
    methods.push(match level {
        "store" => sevenz_rust2::EncoderMethod::COPY.into(),
        "max" => Lzma2Options::from_level(9).into(),
        _ => Lzma2Options::from_level(5).into(),
    });
    w.set_content_methods(methods);
    if !password.is_empty() && hide_names {
        w.set_encrypt_header(true);
    }
    for s in srcs {
        ctx.check_cancel()?;
        let entry = sevenz_rust2::ArchiveEntry::from_path(&s.path, s.name.clone());
        let reader = std::fs::File::open(&s.path).map_err(super::io_err)?;
        w.push_archive_entry(entry, Some(reader)).map_err(bad)?;
        tick(ctx, file_size(&s.path))?;
    }
    w.finish().map_err(super::io_err)?;
    Ok(())
}

fn write_targz(ctx: &Ctx, srcs: &[Source], out: &Path, level: &str, tick: &mut Tick) -> Result<(), FileError> {
    let file = std::fs::File::create(out).map_err(super::io_err)?;
    let lvl = match level {
        "store" => flate2::Compression::none(),
        "max" => flate2::Compression::best(),
        _ => flate2::Compression::default(),
    };
    let gz = flate2::write::GzEncoder::new(std::io::BufWriter::new(file), lvl);
    let mut b = tar::Builder::new(gz);
    b.mode(tar::HeaderMode::Complete);
    for s in srcs {
        ctx.check_cancel()?;
        // Names beyond ASCII also go in a PAX header, read everywhere as UTF-8.
        if !s.name.is_ascii() {
            b.append_pax_extensions([("path", s.name.as_bytes())]).map_err(super::io_err)?;
        }
        b.append_path_with_name(&s.path, &s.name).map_err(super::io_err)?;
        tick(ctx, file_size(&s.path))?;
    }
    b.into_inner().map_err(super::io_err)?.finish().map_err(super::io_err)?.flush().map_err(super::io_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside() {
        assert_eq!(safe_relative("a/b.txt"), Some(PathBuf::from("a").join("b.txt")));
        assert_eq!(safe_relative("../evil.txt"), None);
        assert_eq!(safe_relative("a/../../evil.txt"), None);
        assert_eq!(safe_relative("/etc/passwd"), Some(PathBuf::from("etc").join("passwd")));
        assert_eq!(safe_relative("C:\\Windows\\x.dll"), Some(PathBuf::from("C_").join("Windows").join("x.dll")));
        assert_eq!(safe_relative("CON.txt"), Some(PathBuf::from("_CON.txt")));
        assert_eq!(safe_relative("dir./name. "), Some(PathBuf::from("dir").join("name")));
        assert_eq!(safe_relative("a?b*.txt"), Some(PathBuf::from("a_b_.txt")));
    }

    #[test]
    fn bombs_stop_at_the_limit() {
        assert_eq!(unpack_limit(1000), 4 << 30);
        assert_eq!(unpack_limit(10 << 30), 10_000 << 30);
        assert_eq!(unpack_limit(u64::MAX), u64::MAX);
        let dir = std::env::temp_dir().join(format!("mqkit-bomb-{}", std::process::id()));
        let mut g = Guard::new(&dir, 1 << 20);
        assert_eq!(g.check_declared(u64::MAX).unwrap_err().code, "archive_bomb");
        assert!(g.check_declared(1000).is_ok());
        // Zeros that never end: stopped right after the limit, the partial file removed.
        let to = dir.join("zeros.bin");
        let e = g.write(&mut std::io::repeat(0), &to).unwrap_err();
        assert_eq!(e.code, "archive_bomb");
        assert!(!to.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stems() {
        assert_eq!(archive_stem(Path::new("photos.tar.gz")), "photos");
        assert_eq!(archive_stem(Path::new("Backup 2026.7z.001")), "Backup 2026");
        assert_eq!(archive_stem(Path::new("v1.2.zip")), "v1.2");
    }
}
