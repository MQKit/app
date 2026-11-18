//! Remove metadata (camera, GPS location, author, dates, comments) from photos, PDFs,
//! Office documents and audio/video, without re-encoding the content.

use super::video_compress::VIDEO_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{ffmpeg, pdf};
use contract::{codes, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::path::Path;

pub struct MetaStrip;

const IMAGES: &[&str] = &["jpg", "jpeg", "jfif", "png", "webp"];
/// iPhone photos (HEIC), other HEIF and AVIF: the same ISO container.
const HEIF: &[&str] = &["heic", "heif", "hif", "avif"];
const OFFICE: &[&str] = &["docx", "xlsx", "pptx"];
const AUDIO: &[&str] = &["mp3", "m4a", "flac", "ogg", "opus", "wav"];

impl Plugin for MetaStrip {
    fn id(&self) -> &'static str {
        "meta-strip"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = IMAGES.iter().chain(HEIF).chain(["pdf"].iter()).chain(OFFICE).chain(AUDIO).chain(VIDEO_INPUTS).copied().collect();
        let media: Vec<&str> = AUDIO.iter().chain(VIDEO_INPUTS).copied().collect();
        json!({
            "name": t("Ștergere metadate", "Remove metadata"),
            "summary": t("Locație, autor, cameră", "Location, author, camera"),
            "keywords": { "ro": ["metadate", "exif", "gps", "locație", "autor", "confidențialitate"], "en": ["metadata", "exif", "gps", "location", "author", "privacy"] },
            "category": "other",
            "icon": "eraser",
            // FFmpeg only for music and video (photos, PDFs and documents are cleaned without it).
            "requires": [ { "engine": "ffmpeg", "when": { "file": "extension", "in": media, "quant": "any" } } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": inputs, "replaceOriginalAllowed": true },
            "analysis": { "fast": [] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "keepColorProfile", "type": "toggle", "default": true, "label": t("Păstrează profilul de culoare (pozele)", "Keep the color profile (photos)"),
                  "help": t("Fără el, unele poze arată altfel. Nu conține date personale.", "Without it, some photos look different. It holds no personal data.") },
                { "key": "keepOrientation", "type": "toggle", "default": true, "label": t("Păstrează orientarea (pozele)", "Keep the orientation (photos)"),
                  "help": t("Altfel, pozele făcute cu telefonul pot apărea culcate.", "Otherwise, phone photos may show up sideways.") },
                { "key": "keepTitle", "type": "toggle", "default": false, "label": t("Păstrează titlurile (muzică, video)", "Keep titles (music, video)") }
            ],
            "rules": [
                { "id": "what", "severity": "info", "when": { "any": [ { "field": "keepTitle", "eq": true }, { "field": "keepTitle", "eq": false } ] },
                  "message": t("Se șterg: camera, locația GPS, data, autorul, compania, comentariile. Conținutul nu se recodează.",
                               "Removed: camera, GPS location, date, author, company, comments. The content isn't re-encoded.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_curat", "{name}_clean") } },
            "errors": {}
        })
    }

    fn analyze(&self, _f: &FileRef, _req: &Request) -> Analysis {
        Analysis::ok(Map::new())
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "write", 1, 1);
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let out = ctx.file_dir(&f.id)?.join(format!("result.{ext}"));
        ctx.progress(&f.id, 0.3, "write", 1, 1);
        let removed: Value = match ext.as_str() {
            "jpg" | "jpeg" | "jfif" => {
                let data = std::fs::read(path).map_err(super::io_err)?;
                let (clean, n) = strip_jpeg(&data, req.bool_or("keepColorProfile", true), req.bool_or("keepOrientation", true))?;
                write_file(&out, &clean)?;
                json!({ "segments": n })
            }
            "png" => {
                let data = std::fs::read(path).map_err(super::io_err)?;
                let (clean, n) = strip_png(&data, req.bool_or("keepColorProfile", true))?;
                write_file(&out, &clean)?;
                json!({ "chunks": n })
            }
            "webp" => {
                let data = std::fs::read(path).map_err(super::io_err)?;
                let (clean, n) = strip_webp(&data, req.bool_or("keepColorProfile", true))?;
                write_file(&out, &clean)?;
                json!({ "chunks": n })
            }
            "heic" | "heif" | "hif" | "avif" => {
                let mut data = std::fs::read(path).map_err(super::io_err)?;
                let n = strip_heif(&mut data, req.bool_or("keepOrientation", true))?;
                write_file(&out, &data)?;
                json!({ "items": n })
            }
            "pdf" => {
                let mut doc = pdf::open_file(f, req)?;
                doc.trailer.remove(b"Info");
                if let Ok(cat) = doc.catalog_mut() {
                    cat.remove(b"Metadata");
                }
                pdf::save(&mut doc, &out, false)?;
                json!({})
            }
            "docx" | "xlsx" | "pptx" => {
                strip_office(path, &out)?;
                json!({})
            }
            _ => {
                let keep_title = req.bool_or("keepTitle", false);
                let mut a: Vec<String> = vec!["-i".into(), path.to_string_lossy().into_owned(), "-map".into(), "0".into(), "-c".into(), "copy".into()];
                a.extend(["-map_metadata".into(), "-1".into(), "-map_chapters".into(), "-1".into()]);
                for s in ["v", "a", "s"] {
                    a.extend([format!("-map_metadata:s:{s}"), "-1".into()]);
                }
                if keep_title {
                    let info = ffmpeg::probe(req, path).ok();
                    if let Some(title) = info.as_ref().and_then(|_| title_of(req, path)) {
                        a.extend(["-metadata".into(), format!("title={title}")]);
                    }
                }
                if matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "m4a") {
                    a.extend(["-movflags".into(), "+faststart".into()]);
                }
                a.extend(["-fflags".into(), "+bitexact".into(), "-flags:v".into(), "+bitexact".into(), "-flags:a".into(), "+bitexact".into()]);
                a.push(out.to_string_lossy().into_owned());
                let d = ffmpeg::probe(req, path).ok().and_then(|p| p.get("durationSec").and_then(Value::as_f64)).unwrap_or(0.0);
                ffmpeg::run(ctx, &f.id, &a, d, 1, 1)?;
                json!({})
            }
        };
        ctx.progress(&f.id, 1.0, "write", 1, 1);
        ctx.emit_output(&f.id, &out, &ext, json!({}), in_bytes);
        Ok(removed)
    }
}

fn title_of(req: &Request, path: &Path) -> Option<String> {
    let exe = ffmpeg::exe(req, "ffprobe").ok()?;
    let mut child = ffmpeg::command(&exe)
        .args(["-v", "error", "-show_entries", "format_tags=title", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let (_, stdout, _) = crate::common::proc::output(&mut child, "ffprobe", std::time::Duration::from_secs(90)).ok()?;
    let s = String::from_utf8_lossy(&stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn bad(msg: &str) -> FileError {
    FileError::with(codes::CORRUPT_FILE, json!({ "message": msg }))
}

/// JPEG: drops APP1 (EXIF/XMP), APP12–APP15, APP13 (IPTC) and comments; keeps the
/// picture untouched. With `orientation`, a tiny EXIF holding only the orientation stays.
fn strip_jpeg(d: &[u8], keep_icc: bool, keep_orientation: bool) -> Result<(Vec<u8>, usize), FileError> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return Err(bad("not a JPEG"));
    }
    let mut out = vec![0xFF, 0xD8];
    let mut removed = 0;
    let mut orientation = None;
    let mut i = 2;
    while i + 2 <= d.len() {
        if d[i] != 0xFF {
            return Err(bad("broken JPEG marker"));
        }
        let m = d[i + 1];
        // Fill bytes: any number of 0xFF may come before a marker.
        if m == 0xFF {
            i += 1;
            continue;
        }
        // Markers without a length: TEM and the restart markers (and a repeated SOI).
        if m == 0x01 || m == 0xD8 || (0xD0..=0xD7).contains(&m) {
            out.extend_from_slice(&d[i..i + 2]);
            i += 2;
            continue;
        }
        if m == 0xD9 || m == 0x00 {
            return Err(bad("no picture data"));
        }
        if m == 0xDA {
            // Start of scan: the rest is picture data.
            if let (true, Some(o)) = (keep_orientation, orientation) {
                let app1 = orientation_exif(o);
                // Right after SOI/APP0 is the usual place; inserting before SOS is also valid.
                out.extend_from_slice(&app1);
            }
            out.extend_from_slice(&d[i..]);
            return Ok((out, removed));
        }
        let len_bytes = d.get(i + 2..i + 4).ok_or_else(|| bad("truncated JPEG"))?;
        let len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
        // The length counts its own two bytes: less is a broken file, not an empty segment.
        if len < 2 {
            return Err(bad("broken JPEG segment"));
        }
        let seg = d.get(i..i + 2 + len).ok_or_else(|| bad("truncated JPEG"))?;
        let body = &seg[4..];
        let drop = match m {
            0xE1 => {
                if body.starts_with(b"Exif\0\0") {
                    orientation = exif_orientation(&body[6..]).filter(|o| *o != 1);
                }
                true
            }
            0xE2 => !(keep_icc && body.starts_with(b"ICC_PROFILE\0")),
            0xED | 0xEC | 0xEE | 0xEF | 0xFE => true,
            0xE3..=0xEB => true,
            _ => false,
        };
        if drop {
            removed += 1;
        } else {
            out.extend_from_slice(seg);
        }
        i += 2 + len;
    }
    Err(bad("no picture data"))
}

/// The orientation tag (0x0112) from TIFF-format EXIF.
fn exif_orientation(t: &[u8]) -> Option<u16> {
    let le = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |p: usize| t.get(p..p + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |p: usize| t.get(p..p + 4).map(|b| if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) });
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    for k in 0..n {
        let e = ifd + 2 + k * 12;
        if u16_at(e)? == 0x0112 {
            return u16_at(e + 8);
        }
    }
    None
}

/// An APP1 segment with an EXIF that holds only the orientation.
fn orientation_exif(o: u16) -> Vec<u8> {
    let mut tiff: Vec<u8> = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&o.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut seg = vec![0xFF, 0xE1];
    let len = (2 + 6 + tiff.len()) as u16;
    seg.extend_from_slice(&len.to_be_bytes());
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(&tiff);
    seg
}

/// PNG: drops text chunks (tEXt, zTXt, iTXt), eXIf and tIME; keeps iCCP if asked.
fn strip_png(d: &[u8], keep_icc: bool) -> Result<(Vec<u8>, usize), FileError> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !d.starts_with(SIG) {
        return Err(bad("not a PNG"));
    }
    let mut out = SIG.to_vec();
    let mut removed = 0;
    let mut i = 8;
    while i + 12 <= d.len() {
        let len = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let kind = &d[i + 4..i + 8];
        let chunk = d.get(i..i + 12 + len).ok_or_else(|| bad("truncated PNG"))?;
        let drop = matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") || (kind == b"iCCP" && !keep_icc);
        if drop {
            removed += 1;
        } else {
            out.extend_from_slice(chunk);
        }
        i += 12 + len;
        if kind == b"IEND" {
            break;
        }
    }
    Ok((out, removed))
}

/// WebP: drops the EXIF and XMP chunks (and ICCP if asked), fixing the VP8X flags.
fn strip_webp(d: &[u8], keep_icc: bool) -> Result<(Vec<u8>, usize), FileError> {
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WEBP" {
        return Err(bad("not a WebP"));
    }
    let mut body = Vec::new();
    let mut removed = 0;
    let mut i = 12;
    while i + 8 <= d.len() {
        let kind = &d[i..i + 4];
        let len = u32::from_le_bytes([d[i + 4], d[i + 5], d[i + 6], d[i + 7]]) as usize;
        let padded = len + (len & 1);
        let chunk = d.get(i..(i + 8 + padded).min(d.len())).ok_or_else(|| bad("truncated WebP"))?;
        let drop = kind == b"EXIF" || kind == b"XMP " || (kind == b"ICCP" && !keep_icc);
        if drop {
            removed += 1;
        } else if kind == b"VP8X" && chunk.len() >= 9 {
            let mut c = chunk.to_vec();
            c[8] &= !(0x08 | 0x04); // EXIF and XMP flags
            if !keep_icc {
                c[8] &= !0x20;
            }
            body.extend_from_slice(&c);
        } else {
            body.extend_from_slice(chunk);
        }
        i += 8 + padded;
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Ok((out, removed))
}

/// One ISO-BMFF box: its type and where its body and the box end.
struct Bx {
    kind: [u8; 4],
    body: usize,
    end: usize,
}

fn be(d: &[u8], at: usize, n: usize) -> Option<u64> {
    let b = d.get(at..at.checked_add(n)?)?;
    Some(b.iter().fold(0u64, |v, x| (v << 8) | *x as u64))
}

/// The boxes between `from` and `to`.
fn boxes(d: &[u8], from: usize, to: usize) -> Option<Vec<Bx>> {
    let mut out = Vec::new();
    let mut i = from;
    while i + 8 <= to {
        let size = be(d, i, 4)? as usize;
        let kind: [u8; 4] = d.get(i + 4..i + 8)?.try_into().ok()?;
        let (body, end) = match size {
            0 => (i + 8, to),
            1 => (i + 16, i.checked_add(be(d, i + 8, 8)? as usize)?),
            n => (i + 8, i.checked_add(n)?),
        };
        if end > to || body > end {
            return None;
        }
        out.push(Bx { kind, body, end });
        i = end;
    }
    Some(out)
}

/// The bytes of a NUL-terminated string at `at`, and where it ends.
fn cstr(d: &[u8], at: usize, end: usize) -> Option<(&[u8], usize)> {
    let s = d.get(at..end)?;
    let n = s.iter().position(|b| *b == 0).unwrap_or(s.len());
    Some((&s[..n], (at + n + 1).min(end)))
}

/// HEIC/HEIF/AVIF: the EXIF item (camera, GPS location, dates) and the XMP items are
/// overwritten where they lie, at the same size, so no offset in the file changes and
/// the picture stays byte for byte. The EXIF keeps only the orientation, if asked.
fn strip_heif(d: &mut [u8], keep_orientation: bool) -> Result<usize, FileError> {
    let broken = || bad("broken HEIF/AVIF structure");
    let top = boxes(d, 0, d.len()).ok_or_else(broken)?;
    if top.first().map(|b| &b.kind) != Some(b"ftyp") {
        return Err(bad("not a HEIF/AVIF picture"));
    }
    let meta = top.iter().find(|b| &b.kind == b"meta").ok_or_else(broken)?;
    let parts = boxes(d, meta.body + 4, meta.end).ok_or_else(broken)?;
    // Which items are EXIF or XMP.
    let mut wanted: Vec<(u64, bool)> = Vec::new(); // (item id, is EXIF)
    if let Some(iinf) = parts.iter().find(|b| &b.kind == b"iinf") {
        let skip = if *d.get(iinf.body).ok_or_else(broken)? == 0 { 6 } else { 8 };
        for infe in boxes(d, iinf.body + skip, iinf.end).ok_or_else(broken)?.iter().filter(|b| &b.kind == b"infe") {
            let v = *d.get(infe.body).ok_or_else(broken)?;
            if v < 2 {
                continue;
            }
            let id_len = if v == 2 { 2 } else { 4 };
            let id = be(d, infe.body + 4, id_len).ok_or_else(broken)?;
            let t = infe.body + 4 + id_len + 2;
            let kind = d.get(t..t + 4).ok_or_else(broken)?;
            if kind == b"Exif" {
                wanted.push((id, true));
            } else if kind == b"mime" {
                let (_, next) = cstr(d, t + 4, infe.end).ok_or_else(broken)?;
                let (ctype, _) = cstr(d, next, infe.end).ok_or_else(broken)?;
                if ctype.windows(7).any(|w| w == b"rdf+xml") {
                    wanted.push((id, false));
                }
            }
        }
    }
    if wanted.is_empty() {
        return Ok(0);
    }
    let iloc = parts.iter().find(|b| &b.kind == b"iloc").ok_or_else(broken)?;
    let idat = parts.iter().find(|b| &b.kind == b"idat").map(|b| b.body);
    // Where each wanted item's bytes are.
    let v = *d.get(iloc.body).ok_or_else(broken)?;
    let mut at = iloc.body + 4;
    let sizes = be(d, at, 2).ok_or_else(broken)? as usize;
    let (off_size, len_size, base_size, index_size) = (sizes >> 12, (sizes >> 8) & 15, (sizes >> 4) & 15, if v >= 1 { sizes & 15 } else { 0 });
    at += 2;
    let count_len = if v < 2 { 2 } else { 4 };
    let count = be(d, at, count_len).ok_or_else(broken)?;
    at += count_len;
    let mut places: Vec<(bool, Vec<(usize, usize)>)> = Vec::new();
    for _ in 0..count {
        let id = be(d, at, count_len).ok_or_else(broken)?;
        at += count_len;
        let method = if v >= 1 {
            at += 2;
            be(d, at - 2, 2).ok_or_else(broken)? & 15
        } else {
            0
        };
        at += 2; // data_reference_index
        let base = be(d, at, base_size).ok_or_else(broken)? as usize;
        at += base_size;
        let extents = be(d, at, 2).ok_or_else(broken)?;
        at += 2;
        let mut spans = Vec::new();
        for _ in 0..extents {
            at += index_size;
            let off = be(d, at, off_size).ok_or_else(broken)? as usize;
            at += off_size;
            let len = be(d, at, len_size).ok_or_else(broken)? as usize;
            at += len_size;
            let start = match (method, idat) {
                (0, _) => base.checked_add(off),
                (1, Some(i)) => i.checked_add(base).and_then(|x| x.checked_add(off)),
                _ => None,
            };
            if let Some(s) = start.filter(|s| len > 0 && s.checked_add(len).is_some_and(|e| e <= d.len())) {
                spans.push((s, len));
            }
        }
        if let Some((_, exif)) = wanted.iter().find(|(w, _)| *w == id) {
            places.push((*exif, spans));
        }
    }
    let mut done = 0;
    for (exif, spans) in places {
        let total: usize = spans.iter().map(|s| s.1).sum();
        if total == 0 {
            continue;
        }
        let old: Vec<u8> = spans.iter().flat_map(|(s, l)| d[*s..s + l].to_vec()).collect();
        let new = if exif { heif_exif(&old, keep_orientation) } else { empty_xmp() };
        let mut fill = if new.len() <= total { new } else { Vec::new() };
        // XMP is padded with spaces (allowed before its end), EXIF with zeros.
        let pad = if exif { 0 } else { b' ' };
        const END: usize = r#"<?xpacket end="w"?>"#.len();
        if !exif && fill.len() > END {
            let end = fill.split_off(fill.len() - END);
            fill.resize(total - end.len(), pad);
            fill.extend_from_slice(&end);
        }
        fill.resize(total, pad);
        let mut k = 0;
        for (s, l) in spans {
            d[s..s + l].copy_from_slice(&fill[k..k + l]);
            k += l;
        }
        done += 1;
    }
    Ok(done)
}

/// A HEIF EXIF item (a 4-byte offset, then "Exif\0\0" and TIFF) holding at most the orientation.
fn heif_exif(old: &[u8], keep_orientation: bool) -> Vec<u8> {
    let tiff_at = be(old, 0, 4).map(|o| 4 + o as usize).unwrap_or(4);
    let orientation = old.get(tiff_at..).and_then(exif_orientation).filter(|o| keep_orientation && *o != 1);
    let mut out = vec![0, 0, 0, 6];
    out.extend_from_slice(b"Exif\0\0");
    match orientation {
        Some(o) => out.extend_from_slice(&orientation_exif(o)[10..]),
        None => {
            out.extend_from_slice(b"II*\0");
            out.extend_from_slice(&8u32.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // no entries, no next IFD
        }
    }
    out
}

fn empty_xmp() -> Vec<u8> {
    br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"/><?xpacket end="w"?>"#.to_vec()
}

/// Word, Excel, PowerPoint: blank document properties (author, company, dates…).
fn strip_office(src: &Path, dst: &Path) -> Result<(), FileError> {
    let file = std::fs::File::open(src).map_err(super::io_err)?;
    let mut zin = zip::ZipArchive::new(file).map_err(|e| bad(&e.to_string()))?;
    let out = std::fs::File::create(dst).map_err(super::io_err)?;
    let mut zout = zip::ZipWriter::new(out);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    // The parts that hold names are rewritten; everything else (pictures, a video in a
    // presentation) is copied as it is stored, without unpacking it into memory.
    const XML_LIMIT: u64 = 256 * 1024 * 1024;
    for k in 0..zin.len() {
        let name = zin.by_index_raw(k).map_err(|e| bad(&e.to_string()))?.name().map_err(|e| bad(&e.to_string()))?.to_string();
        if !(name.starts_with("docProps/") || people_part(&name)) {
            let raw = zin.by_index_raw(k).map_err(|e| bad(&e.to_string()))?;
            zout.raw_copy_file(raw).map_err(|e| match e {
                zip::result::ZipError::Io(io) => super::io_err(io),
                e => bad(&e.to_string()),
            })?;
            continue;
        }
        let entry = zin.by_index(k).map_err(|e| bad(&e.to_string()))?;
        let mut data = Vec::new();
        entry.take(XML_LIMIT + 1).read_to_end(&mut data).map_err(super::io_err)?;
        // A part this big is not a real document part (a zip bomb).
        if data.len() as u64 > XML_LIMIT {
            return Err(FileError::with(codes::FILE_TOO_LARGE, json!({ "part": name })));
        }
        let data = match name.as_str() {
            "docProps/core.xml" => br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"></cp:coreProperties>"#
                .to_vec(),
            "docProps/app.xml" => clear_tags(&data, &["Company", "Manager", "HyperlinkBase"]),
            "docProps/custom.xml" => br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"></Properties>"#
                .to_vec(),
            n if n.ends_with(".xml") => anonymize_people(n, data),
            _ => data,
        };
        zout.start_file(name, opts).map_err(|e| bad(&e.to_string()))?;
        zout.write_all(&data).map_err(super::io_err)?;
    }
    zout.finish().map_err(|e| bad(&e.to_string()))?;
    Ok(())
}

/// The people in a document: who wrote each comment and each tracked change, and the
/// lists of them (with their account IDs and e-mail addresses). Everyone becomes
/// "Author", as Office's own "Remove personal information" does.
/// The XML parts [`anonymize_people`] changes.
fn people_part(name: &str) -> bool {
    name.ends_with(".xml")
        && (name.starts_with("word/")
            || name.starts_with("xl/persons/")
            || name == "ppt/commentAuthors.xml"
            || name == "ppt/authors.xml"
            || name.starts_with("xl/comments"))
}

fn anonymize_people(name: &str, data: Vec<u8>) -> Vec<u8> {
    let attrs: &[(&str, &str)] = if name.starts_with("word/") {
        &[("w:author", "Author"), ("w:initials", "A"), ("w15:author", "Author"), ("w15:userId", ""), ("w15:providerId", "None")]
    } else if name.starts_with("xl/persons/") {
        &[("displayName", "Author"), ("userId", ""), ("providerId", "None")]
    } else if name == "ppt/commentAuthors.xml" || name == "ppt/authors.xml" {
        &[("name", "Author"), ("initials", "A"), ("userId", ""), ("providerId", "None")]
    } else if name.starts_with("xl/comments") {
        &[]
    } else {
        return data;
    };
    let mut s = String::from_utf8_lossy(&data).into_owned();
    for (attr, value) in attrs {
        s = replace_attr(&s, attr, value);
    }
    if name.starts_with("xl/comments") {
        // <authors><author>Ana</author>…: the comments point to them by position.
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(a) = rest.find("<author>") {
            let Some(b) = rest[a..].find("</author>").map(|i| a + i) else { break };
            out.push_str(&rest[..a]);
            out.push_str("<author>Author</author>");
            rest = &rest[b + "</author>".len()..];
        }
        out.push_str(rest);
        s = out;
    }
    s.into_bytes()
}

/// ` attr="anything"` → ` attr="value"` everywhere (the attribute itself, not one whose
/// name merely ends the same way).
fn replace_attr(xml: &str, attr: &str, value: &str) -> String {
    let key = format!("{attr}=\"");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find(&key) {
        let before_ok = rest[..i].chars().last().is_some_and(char::is_whitespace);
        let start = i + key.len();
        let Some(end) = rest[start..].find('"').map(|e| start + e) else { break };
        out.push_str(&rest[..start]);
        out.push_str(if before_ok { value } else { &rest[start..end] });
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `<Tag>anything</Tag>` → `<Tag></Tag>` for each tag.
fn clear_tags(xml: &[u8], tags: &[&str]) -> Vec<u8> {
    let mut s = String::from_utf8_lossy(xml).to_string();
    for tag in tags {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        if let (Some(a), Some(b)) = (s.find(&open), s.find(&close)) {
            if b > a {
                s.replace_range(a + open.len()..b, "");
            }
        }
    }
    s.into_bytes()
}

#[cfg(test)]
mod people_tests {
    #[test]
    fn authors_become_author() {
        let x = br#"<w:comment w:id="0" w:author="Steve Canny" w:date="2025-06-07T00:00:00Z" w:initials="SJC"><w:p/></w:comment><w:ins w:author="Ana"/>"#.to_vec();
        let out = String::from_utf8(super::anonymize_people("word/comments.xml", x)).unwrap();
        assert!(!out.contains("Steve") && !out.contains("SJC") && !out.contains("Ana"));
        assert!(out.contains(r#"w:author="Author""#) && out.contains(r#"w:date="2025-06-07T00:00:00Z""#));
        let x = b"<comments><authors><author>Ion</author><author>Ana</author></authors></comments>".to_vec();
        assert_eq!(String::from_utf8(super::anonymize_people("xl/comments1.xml", x)).unwrap(), "<comments><authors><author>Author</author><author>Author</author></authors></comments>");
        let x = br#"<p:cNvPr id="2" name="Title 1"/>"#.to_vec();
        assert_eq!(super::anonymize_people("ppt/slides/slide1.xml", x.clone()), x);
    }
}

#[cfg(test)]
mod jpeg_tests {
    use super::strip_jpeg;

    fn seg(m: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, m];
        v.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    fn has(d: &[u8], w: &[u8]) -> bool {
        d.windows(w.len()).any(|x| x == w)
    }

    const SCAN: &[u8] = &[0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xD9];

    #[test]
    fn exif_after_fill_bytes_is_removed() {
        let mut d = vec![0xFF, 0xD8];
        d.extend(seg(0xE0, b"JFIF\0"));
        d.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
        d.extend(seg(0xE1, b"Exif\0\0SECRETGPS"));
        d.extend(seg(0xFE, b"a comment"));
        d.extend_from_slice(SCAN);
        let (out, removed) = strip_jpeg(&d, true, false).unwrap();
        assert_eq!(removed, 2);
        assert!(!has(&out, b"SECRETGPS") && !has(&out, b"a comment"));
        assert!(has(&out, b"JFIF") && out.ends_with(SCAN));
    }

    #[test]
    fn markers_without_length_are_kept() {
        let mut d = vec![0xFF, 0xD8, 0xFF, 0x01, 0xFF, 0xD0];
        d.extend(seg(0xE1, b"Exif\0\0SECRET"));
        d.extend_from_slice(SCAN);
        let (out, removed) = strip_jpeg(&d, true, false).unwrap();
        assert_eq!(removed, 1);
        assert!(out.starts_with(&[0xFF, 0xD8, 0xFF, 0x01, 0xFF, 0xD0]) && !has(&out, b"SECRET"));
    }

    #[test]
    fn broken_lengths_are_errors_not_panics() {
        for len in [0u8, 1] {
            let mut d = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, len];
            d.extend_from_slice(SCAN);
            assert!(strip_jpeg(&d, true, false).is_err());
        }
        assert!(strip_jpeg(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00], true, false).is_err());
        assert!(strip_jpeg(&[0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFF, 1, 2], true, false).is_err());
        assert!(strip_jpeg(&[0xFF, 0xD8, 0xFF, 0xD9], true, false).is_err());
    }
}

#[cfg(test)]
mod heif_tests {
    fn bx(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn heic_exif_and_xmp_erased_in_place() {
        // EXIF item 1 (orientation 6 and a camera name), XMP item 2, picture item 3, all in mdat.
        let mut tiff = b"MM\0*\0\0\0\x08\0\x02".to_vec();
        tiff.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        tiff.extend_from_slice(&[0x01, 0x0F, 0, 2, 0, 0, 0, 4, b'C', b'a', b'm', 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        let mut exif = vec![0, 0, 0, 6];
        exif.extend_from_slice(b"Exif\0\0");
        exif.extend_from_slice(&tiff);
        let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><exif:GPSLatitude>44,25.5N</exif:GPSLatitude></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#.to_vec();
        let picture = b"PICTUREDATA".to_vec();
        let infe = |id: u16, kind: &[u8], extra: &[u8]| {
            let mut b = vec![2, 0, 0, 0];
            b.extend_from_slice(&id.to_be_bytes());
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(kind);
            b.push(0);
            b.extend_from_slice(extra);
            bx(b"infe", &b)
        };
        let mut iinf = vec![0, 0, 0, 0, 0, 3];
        iinf.extend(infe(1, b"Exif", b""));
        iinf.extend(infe(2, b"mime", b"application/rdf+xml\0"));
        iinf.extend(infe(3, b"hvc1", b""));
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let iloc_for = |base: u32| {
            let mut b = vec![0, 0, 0, 0, 0x44, 0x00, 0, 3];
            let mut at = base;
            for (id, len) in [(1u16, exif.len()), (2, xmp.len()), (3, picture.len())] {
                b.extend_from_slice(&id.to_be_bytes());
                b.extend_from_slice(&[0, 0, 0, 1]);
                b.extend_from_slice(&at.to_be_bytes());
                b.extend_from_slice(&(len as u32).to_be_bytes());
                at += len as u32;
            }
            bx(b"iloc", &b)
        };
        let meta_len = |iloc: &[u8]| 8 + 4 + bx(b"iinf", &iinf).len() + iloc.len();
        let probe = iloc_for(0);
        let data_at = (ftyp.len() + meta_len(&probe) + 8) as u32;
        let mut meta = vec![0, 0, 0, 0];
        meta.extend(bx(b"iinf", &iinf));
        meta.extend(iloc_for(data_at));
        let mut mdat = exif.clone();
        mdat.extend_from_slice(&xmp);
        mdat.extend_from_slice(&picture);
        let mut file = ftyp.clone();
        file.extend(bx(b"meta", &meta));
        file.extend(bx(b"mdat", &mdat));
        let original = file.clone();

        assert_eq!(super::strip_heif(&mut file, true).unwrap(), 2);
        assert_eq!(file.len(), original.len());
        let has = |d: &[u8], w: &[u8]| d.windows(w.len()).any(|x| x == w);
        assert!(!has(&file, b"Cam") && !has(&file, b"GPSLatitude"));
        assert!(file.ends_with(b"PICTUREDATA"));
        let e = data_at as usize;
        assert_eq!(super::exif_orientation(&file[e + 10..]), Some(6));
        let x = &file[e + exif.len()..e + exif.len() + xmp.len()];
        assert!(x.starts_with(b"<?xpacket") && x.ends_with(br#"<?xpacket end="w"?>"#));

        let mut again = original.clone();
        super::strip_heif(&mut again, false).unwrap();
        assert_eq!(super::exif_orientation(&again[e + 10..]), None);
        assert!(super::strip_heif(&mut b"not a picture at all".to_vec(), true).is_err());
    }
}
