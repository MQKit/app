//! Tool 1: compress PDF. Recompresses and downsamples the images (JPEG via mozjpeg),
//! then rewrites the structure (object streams, unused objects, thumbnails).
//! Text and vector graphics are never touched, so forms and text stay intact.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{img, pdf};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{DynamicImage, GrayImage, RgbImage};
use lopdf::{Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct PdfCompress;

#[derive(Clone, Copy, Debug)]
struct Settings {
    dpi: f64,
    quality: u8,
    gray: bool,
    images: bool,
}

const LEVELS: [(&str, f64, u8); 4] = [("light", 220.0, 85), ("recommended", 150.0, 72), ("strong", 110.0, 58), ("extreme", 80.0, 42)];

fn level(name: &str) -> (f64, u8) {
    LEVELS.iter().find(|l| l.0 == name).map(|l| (l.1, l.2)).unwrap_or((150.0, 72))
}

fn settings(req: &Request) -> Settings {
    let gray = req.str_or("colorMode", "keep") == "gray";
    let images = !req.bool_or("losslessOnly", false);
    let (dpi, quality) = match req.str_or("mode", "preset") {
        "custom" => (req.f64_or("imageDpi", 150.0), req.f64_or("imageQuality", 75.0) as u8),
        _ => level(req.str_or("level", "recommended")),
    };
    Settings { dpi, quality, gray, images }
}

impl Plugin for PdfCompress {
    fn id(&self) -> &'static str {
        "pdf-compress"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Comprimare PDF", "Compress PDF"),
            "summary": t("Fă-l mai mic", "Make it smaller"),
            "keywords": { "ro": ["micșorează", "reduce", "mărime", "email", "mic"], "en": ["shrink", "reduce", "size", "email", "smaller"] },
            "category": "pdf",
            "icon": "file-zip",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"],
                        "nameVariables": [ { "id": "pages", "label": t("număr de pagini", "page count") } ],
                        "replaceOriginalAllowed": true },
            "analysis": { "fast": ["needsPassword", "pageCount", "hasForms", "isSigned", "isPdfA", "imageBytesRatio"],
                          "warnings": ["little_to_gain"] },
            "ui": { "preview": "none", "estimate": true },
            "resources": { "class": "heavy", "diskFactor": 1.5 },
            "presets": [
                { "id": "email", "label": t("Pentru e-mail", "For email"), "default": true,
                  "values": { "mode": "targetSize", "targetSize": 10000000 } },
                { "id": "balanced", "label": t("Echilibrat", "Balanced"), "values": { "mode": "preset", "level": "recommended" } },
                { "id": "smallest", "label": t("Cât mai mic", "Smallest"), "values": { "mode": "preset", "level": "extreme" } },
                { "id": "print", "label": t("Pentru tipar", "For print"), "values": { "mode": "preset", "level": "light" } }
            ],
            "groups": [
                { "id": "images", "label": t("Imagini", "Images"), "order": 1 },
                { "id": "document", "label": t("Document", "Document"), "order": 2 }
            ],
            "fields": [
                { "key": "mode", "type": "select", "label": t("Cum comprimăm", "How to compress"), "default": "preset",
                  "options": [ opt("preset", "După nivel", "By level"), opt("targetSize", "Sub o mărime", "Under a size"), opt("custom", "Personalizat", "Custom") ] },
                { "key": "level", "type": "select", "label": t("Nivel", "Level"), "default": "recommended",
                  "showIf": { "field": "mode", "eq": "preset" },
                  "options": [ opt("light", "Calitate maximă", "Best quality"), opt("recommended", "Recomandat", "Recommended"),
                               opt("strong", "Puternic", "Strong"), opt("extreme", "Cât mai mic", "Smallest") ] },
                { "key": "targetSize", "type": "number", "label": t("Mărime maximă", "Maximum size"), "default": 10000000,
                  "min": 100000, "max": 2000000000, "showIf": { "field": "mode", "eq": "targetSize" },
                  "units": { "base": "B", "display": [ { "id": "MB", "factor": 1000000 }, { "id": "KB", "factor": 1000 } ], "defaultDisplay": "MB" },
                  "suggestions": [ { "value": 25000000, "label": t("25 MB (Gmail)", "25 MB (Gmail)") }, { "value": 10000000, "label": t("10 MB", "10 MB") },
                                   { "value": 5000000, "label": t("5 MB", "5 MB") }, { "value": 2000000, "label": t("2 MB", "2 MB") } ] },
                { "key": "colorMode", "type": "select", "label": t("Culori", "Colors"), "default": "keep",
                  "options": [ opt("keep", "Păstrează culorile", "Keep colors"), opt("gray", "Alb-negru (tonuri de gri)", "Grayscale") ] },
                { "key": "imageDpi", "type": "range", "advanced": true, "group": "images", "label": t("Rezoluție", "Resolution"),
                  "min": 50, "max": 300, "step": 10, "unit": "dpi", "default": 150,
                  "showIf": { "all": [ { "field": "mode", "eq": "custom" }, { "field": "losslessOnly", "eq": false } ] } },
                { "key": "imageQuality", "type": "range", "advanced": true, "group": "images", "label": t("Calitate", "Quality"),
                  "min": 10, "max": 100, "step": 1, "unit": "%", "default": 75,
                  "showIf": { "all": [ { "field": "mode", "eq": "custom" }, { "field": "losslessOnly", "eq": false } ] } },
                { "key": "losslessOnly", "type": "toggle", "advanced": true, "group": "document", "default": false,
                  "label": t("Doar fără pierderi", "Lossless only"),
                  "help": t("Nu atinge imaginile. Câștig mic, dar totul rămâne identic.", "Leaves images alone. Small gain, but everything stays identical.") },
                { "key": "removeThumbnails", "type": "toggle", "advanced": true, "group": "document", "default": true,
                  "label": t("Scoate miniaturile paginilor", "Remove page thumbnails") },
                { "key": "removeMetadata", "type": "toggle", "advanced": true, "group": "document", "default": false,
                  "label": t("Șterge titlul, autorul și celelalte informații", "Remove title, author and other information") },
                { "key": "linearize", "type": "toggle", "advanced": true, "group": "document", "default": false,
                  "label": t("Optimizat pentru afișare pe web", "Fast web view") },
                { "key": "ifLarger", "type": "select", "advanced": true, "group": "document", "default": "keepOriginal",
                  "label": t("Dacă iese mai mare", "If the result is larger"),
                  "options": [ opt("keepOriginal", "Nu scrie nimic", "Don't write anything"), opt("writeAnyway", "Scrie oricum", "Write anyway") ] }
            ],
            "rules": [
                { "id": "signed_warning", "when": { "file": "isSigned", "eq": true }, "severity": "confirm",
                  "message": t("Unele fișiere sunt semnate digital. Semnătura nu va mai fi validă în fișierul comprimat.",
                               "Some files are digitally signed. The signature will no longer be valid in the compressed file.") },
                { "id": "pdfa_lost", "when": { "all": [ { "file": "isPdfA", "eq": true }, { "field": "losslessOnly", "eq": false } ] }, "severity": "info",
                  "message": t("Fișierul nu va mai fi PDF/A.", "The file will no longer be PDF/A.") },
                { "id": "little_images", "when": { "file": "imageBytesRatio", "lt": 0.2, "quant": "all" }, "severity": "info",
                  "message": t("Fișierele au puține imagini; câștigul va fi mic.", "The files have few images; the gain will be small.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_comprimat", "{name}_compressed") } },
            "errors": {
                "target_unreachable": t("Fișierul nu poate ajunge la mărimea cerută; cel mai mic iese {bestBytes}.", "The file can't reach the requested size; the smallest it gets is {bestBytes}."),
                "little_to_gain": t("Fișierul are puține imagini; câștigul e mic.", "The file has few images; the gain is small.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn estimate(&self, req: &Request) -> Option<Value> {
        let mut total_in = 0u64;
        let mut total_out = 0f64;
        let mut reachable = true;
        for f in &req.files {
            let in_bytes = file_size(f.input_path());
            total_in += in_bytes;
            let doc = pdf::open_file(f, req).ok()?;
            let structure = restructured_size(&doc, in_bytes);
            let size_with = |s: Settings| -> f64 {
                let ratio = if s.images { sample_ratio(&doc, s) } else { 1.0 };
                match structure {
                    Some((size, images)) => size - images * (1.0 - ratio),
                    None => {
                        let img_bytes = image_bytes(&doc) as f64;
                        (in_bytes as f64 - img_bytes).max(0.0) * 0.85 + img_bytes * ratio
                    }
                }
                .min(in_bytes as f64)
            };
            let base = settings(req);
            let est = if req.str_or("mode", "preset") == "targetSize" {
                let target = req.f64_or("targetSize", 1e7);
                let usual = size_with(Settings { dpi: 150.0, quality: 72, ..base });
                if usual <= target {
                    usual
                } else {
                    // The run steps down to its smallest attempt; if even that is too big,
                    // say how small it can get rather than promising the target.
                    let smallest = size_with(Settings { dpi: 60.0, quality: 30, ..base });
                    if smallest > target {
                        reachable = false;
                        smallest
                    } else {
                        target
                    }
                }
            } else {
                size_with(base)
            };
            total_out += est;
        }
        Some(json!({ "inputBytes": total_in, "outputBytes": total_out.round(), "confidence": if reachable { "medium" } else { "low" }, "method": "sample" }))
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let in_bytes = file_size(f.input_path());
        ctx.progress(&f.id, 0.0, "analyze", 1, 3);
        let original = pdf::open_file(f, ctx.req)?;
        let pages = original.get_pages().len();
        let dir = ctx.file_dir(&f.id)?;
        let out = dir.join("result.pdf");
        let base = settings(ctx.req);

        #[allow(clippy::needless_late_init)] // set in each branch below, after work that may fail
        let resolved;
        if ctx.req.str_or("mode", "preset") == "targetSize" && base.images {
            let target = ctx.req.f64_or("targetSize", 1e7) as u64;
            let mut attempts: Vec<Settings> = LEVELS.iter().skip(1).map(|l| Settings { dpi: l.1, quality: l.2, ..base }).collect();
            attempts.push(Settings { dpi: 60.0, quality: 30, ..base });
            let n = attempts.len() + 2;
            let mut best: Option<u64> = None;
            let mut hit: Option<(Settings, u64)> = None;
            let mut prev: Option<Settings> = None;
            let try_one = |i: usize, s: Settings| -> Result<(std::path::PathBuf, u64), FileError> {
                let tmp = dir.join(format!("try{i}.pdf"));
                let size = compress_to(ctx, f, original.clone(), s, &tmp, i as f64 / n as f64, 1.0 / n as f64)?;
                Ok((tmp, size))
            };
            for (i, s) in attempts.into_iter().enumerate() {
                let (tmp, size) = try_one(i, s)?;
                best = Some(best.map_or(size, |b: u64| b.min(size)));
                if size <= target {
                    std::fs::rename(&tmp, &out).map_err(super::io_err)?;
                    hit = Some((s, size));
                    break;
                }
                let _ = std::fs::remove_file(&tmp);
                prev = Some(s);
            }
            // Far under the target: try between the last two levels, to keep more quality.
            if let (Some((mut lo, mut lo_size)), Some(mut hi)) = (hit, prev) {
                for k in 0..2 {
                    if lo_size as f64 > target as f64 * 0.7 {
                        break;
                    }
                    let mid = Settings { dpi: (lo.dpi + hi.dpi) / 2.0, quality: ((lo.quality as u16 + hi.quality as u16) / 2) as u8, ..base };
                    let (tmp, size) = try_one(100 + k, mid)?;
                    if size <= target {
                        std::fs::rename(&tmp, &out).map_err(super::io_err)?;
                        lo = mid;
                        lo_size = size;
                    } else {
                        let _ = std::fs::remove_file(&tmp);
                        hi = mid;
                    }
                }
                hit = Some((lo, lo_size));
            }
            let Some((s, _)) = hit else {
                return Err(FileError::with(codes::TARGET_UNREACHABLE, json!({ "bestBytes": best }))
                    .suggest(json!({ "mode": "preset", "level": "extreme" })));
            };
            resolved = json!({ "dpi": s.dpi, "quality": s.quality });
        } else {
            compress_to(ctx, f, original, base, &out, 0.0, 1.0)?;
            resolved = json!({ "dpi": base.dpi, "quality": base.quality });
        }

        let out_bytes = file_size(&out);
        if out_bytes >= in_bytes && ctx.req.str_or("ifLarger", "keepOriginal") == "keepOriginal" {
            let _ = std::fs::remove_file(&out);
            return Err(FileError::skipped(codes::OUTPUT_LARGER, json!({ "inputBytes": in_bytes, "outputBytes": out_bytes })));
        }
        ctx.progress(&f.id, 1.0, "restructure", 3, 3);
        ctx.emit_output(&f.id, &out, "pdf", json!({ "pages": pages }), in_bytes);
        Ok(resolved)
    }
}

/// One full pass; returns the output size.
fn compress_to(ctx: &Ctx, f: &FileRef, mut doc: Document, s: Settings, out: &std::path::Path, base: f64, span: f64) -> Result<u64, FileError> {
    if s.images {
        recompress_images(ctx, f, &mut doc, s, base, span * 0.85)?;
    }
    ctx.check_cancel()?;
    if s.gray {
        gray_vectors(&mut doc);
    }
    ctx.progress(&f.id, base + span * 0.9, "restructure", 3, 3);
    if ctx.req.bool_or("removeThumbnails", true) {
        let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        for p in pages {
            if let Ok(d) = doc.get_dictionary_mut(p) {
                d.remove(b"Thumb");
            }
        }
    }
    if ctx.req.bool_or("removeMetadata", false) {
        doc.trailer.remove(b"Info");
        if let Ok(c) = doc.catalog_mut() {
            c.remove(b"Metadata");
            c.remove(b"PieceInfo");
        }
    }
    doc.prune_objects();
    doc.delete_zero_length_streams();
    doc.compress();
    pdf::save(&mut doc, out, ctx.req.bool_or("linearize", false))?;
    Ok(file_size(out))
}

/// Filters whose data lopdf can decode (in a chain; DCT only as the last one).
const DECODABLE: [&[u8]; 5] = [b"FlateDecode", b"LZWDecode", b"ASCII85Decode", b"ASCIIHexDecode", b"RunLengthDecode"];

/// Images worth recompressing: RGB or gray, 8 or 16 bits, stored as JPEG or with
/// filters we can decode; not masks, not colour-key masked, not with a /Decode array.
fn candidates(doc: &Document) -> Vec<ObjectId> {
    let masks: HashSet<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .flat_map(|s| [s.dict.get(b"SMask"), s.dict.get(b"Mask")])
        .filter_map(|o| o.ok().and_then(|o| o.as_reference().ok()))
        .collect();
    doc.objects
        .iter()
        .filter(|(id, _)| !masks.contains(id))
        .filter_map(|(id, o)| {
            let s = o.as_stream().ok()?;
            let d = &s.dict;
            if d.get(b"Subtype").and_then(Object::as_name).ok()? != b"Image" {
                return None;
            }
            // A colour-key mask (an array) needs exact colours: JPEG would break it.
            if d.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) || d.has(b"Decode") || matches!(d.get(b"Mask"), Ok(Object::Array(_))) {
                return None;
            }
            let bpc = d.get(b"BitsPerComponent").and_then(Object::as_i64).ok()?;
            components(doc, d.get(b"ColorSpace").ok()?)?;
            let filters = s.filters().ok()?;
            let dct = filters.last().map(|f| *f == b"DCTDecode").unwrap_or(false);
            let rest = if dct { &filters[..filters.len() - 1] } else { &filters[..] };
            if filters.is_empty() || !rest.iter().all(|f| DECODABLE.contains(f)) {
                return None;
            }
            if !(bpc == 8 || (bpc == 16 && !dct)) || s.content.len() < 8_000 {
                return None;
            }
            Some(*id)
        })
        .collect()
}

/// 1 (gray) or 3 (RGB) for supported color spaces.
fn components(doc: &Document, cs: &Object) -> Option<u8> {
    let (_, cs) = doc.dereference(cs).ok()?;
    match cs {
        Object::Name(n) if n == b"DeviceRGB" || n == b"CalRGB" => Some(3),
        Object::Name(n) if n == b"DeviceGray" || n == b"CalGray" => Some(1),
        Object::Array(a) if a.len() == 2 && a[0].as_name().ok()? == b"ICCBased" => {
            let s = doc.dereference(&a[1]).ok()?.1.as_stream().ok()?;
            match s.dict.get(b"N").and_then(Object::as_i64).ok()? {
                3 => Some(3),
                1 => Some(1),
                _ => None,
            }
        }
        Object::Array(a) if a.len() == 2 && a[0].as_name().ok()? == b"CalRGB" => Some(3),
        Object::Array(a) if a.len() == 2 && a[0].as_name().ok()? == b"CalGray" => Some(1),
        _ => None,
    }
}

struct Job {
    w: u32,
    h: u32,
    comps: u8,
    /// JPEG bytes when `dct`, else the decoded samples at 8 bits.
    dct: bool,
    data: Vec<u8>,
    /// The stream's size in the file now.
    old_len: usize,
}

/// The image ready to recompress, or None when it can't be decoded within reason.
fn job(doc: &Document, id: ObjectId, max_pixels: u64) -> Option<Job> {
    let st = doc.get_object(id).and_then(Object::as_stream).ok()?;
    let w = st.dict.get(b"Width").and_then(Object::as_i64).ok()?;
    let h = st.dict.get(b"Height").and_then(Object::as_i64).ok()?;
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > max_pixels {
        return None;
    }
    let (w, h) = (w as u32, h as u32);
    let comps = components(doc, st.dict.get(b"ColorSpace").ok()?)?;
    let bpc = st.dict.get(b"BitsPerComponent").and_then(Object::as_i64).ok()?;
    let filters: Vec<Vec<u8>> = st.filters().ok()?.into_iter().map(<[u8]>::to_vec).collect();
    let dct = filters.last().map(|f| f == b"DCTDecode").unwrap_or(false);
    let expected = w as usize * h as usize * comps as usize * (bpc as usize / 8);
    let data = if dct {
        if filters.len() == 1 {
            st.content.clone()
        } else {
            // e.g. [/ASCII85Decode /DCTDecode]: undo the first filters, keep the JPEG.
            let mut pre = st.clone();
            pre.dict.set("Filter", Object::Array(filters[..filters.len() - 1].iter().map(|f| Object::Name(f.clone())).collect()));
            pre.decompressed_content_with_limit(st.content.len() * 2 + 1024).ok()?
        }
    } else {
        let mut raw = st.decompressed_content_with_limit(expected + 4096).ok()?;
        if raw.len() < expected {
            return None;
        }
        raw.truncate(expected);
        if bpc == 16 {
            // Big-endian samples: the high byte is the 8-bit value.
            raw = raw.chunks_exact(2).map(|c| c[0]).collect();
        }
        raw
    };
    Some(Job { w, h, comps, dct, data, old_len: st.content.len() })
}

fn decode(job: &Job) -> Option<DynamicImage> {
    if job.dct {
        let i = image::load_from_memory_with_format(&job.data, image::ImageFormat::Jpeg).ok()?;
        // CMYK JPEGs are excluded by the color space check; Adobe YCCK too.
        return Some(i);
    }
    let raw = job.data.clone();
    if job.comps == 1 {
        GrayImage::from_raw(job.w, job.h, raw).map(DynamicImage::ImageLuma8)
    } else {
        RgbImage::from_raw(job.w, job.h, raw).map(DynamicImage::ImageRgb8)
    }
}

fn zlib(data: &[u8], level: u32) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// The new image: JPEG or lossless Flate.
struct Shrunk {
    data: Vec<u8>,
    w: u32,
    h: u32,
    gray: bool,
    jpeg: bool,
}

/// Returns the new image when it is clearly smaller than the old stream.
fn shrink(job: &Job, s: Settings, max_px: f64) -> Option<Shrunk> {
    let mut im = decode(job)?;
    let long = im.width().max(im.height()) as f64;
    let mut resized = false;
    if long > max_px * 1.1 {
        let sc = max_px / long;
        let nw = ((im.width() as f64 * sc).round() as u32).max(1);
        let nh = ((im.height() as f64 * sc).round() as u32).max(1);
        im = im.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
        resized = true;
    }
    let gray = s.gray || job.comps == 1;
    if gray {
        im = DynamicImage::ImageLuma8(im.to_luma8());
    }
    let smaller = |n: usize| (n as f64) < job.old_len as f64 * 0.92;
    if !job.dct && !resized && !s.gray {
        // Stored without loss. A photo (deflate barely shrinks it) can become JPEG; a
        // screenshot or drawing would blur, so it only gets a better lossless encoding.
        let flate = zlib(&job.data, 9);
        let photo = flate.len() as f64 > job.data.len() as f64 * 0.30;
        if !photo {
            return smaller(flate.len()).then_some(Shrunk { data: flate, w: job.w, h: job.h, gray: false, jpeg: false });
        }
    }
    let jpg = img::encode_jpeg(&im, s.quality, None, None, [255, 255, 255]).ok()?;
    (smaller(jpg.len()) || (gray && job.comps == 3)).then(|| Shrunk { data: jpg, w: im.width(), h: im.height(), gray, jpeg: true })
}

fn recompress_images(ctx: &Ctx, f: &FileRef, doc: &mut Document, s: Settings, base: f64, span: f64) -> Result<(), FileError> {
    let max_px = max_page_side(doc) / 72.0 * s.dpi;
    let max_pixels = (ctx.req.limits.max_megapixels.max(1) as u64) * 1_000_000;
    // Each thread unpacks the next image itself, so at most one unpacked image per thread
    // is in memory (unpacking them all first held every scan of a big PDF at once).
    let ids = candidates(doc);
    let total = ids.len().max(1);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let threads = (ctx.req.limits.threads.max(1) as usize).min(8).min(ids.len().max(1));
    let shared: &Document = doc;
    let results: Vec<(ObjectId, Option<Shrunk>)> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (ids, next, done) = (&ids, &next, &done);
                sc.spawn(move || {
                    let mut out = Vec::new();
                    while let Some(&id) = ids.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if ctx.cancel.is_cancelled() {
                            break;
                        }
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(shared, id, max_pixels).and_then(|j| shrink(&j, s, max_px))))
                            .ok()
                            .flatten();
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        ctx.progress(&f.id, base + span * n as f64 / total as f64, "compress", 2, 3);
                        if r.is_some() {
                            out.push((id, r));
                        }
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    ctx.check_cancel()?;
    for (id, r) in results {
        let Some(n) = r else { continue };
        if let Ok(Object::Stream(st)) = doc.get_object_mut(id) {
            st.dict.set("Filter", Object::Name(if n.jpeg { b"DCTDecode".to_vec() } else { b"FlateDecode".to_vec() }));
            st.dict.remove(b"DecodeParms");
            st.dict.set("Width", n.w as i64);
            st.dict.set("Height", n.h as i64);
            st.dict.set("BitsPerComponent", 8);
            if n.gray {
                st.dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
            }
            st.set_content(n.data);
            st.allows_compression = false;
        }
    }
    Ok(())
}

/// Estimate: recompress up to 6 of the largest images and return the size ratio.
fn sample_ratio(doc: &Document, s: Settings) -> f64 {
    let mut ids = candidates(doc);
    ids.sort_by_key(|id| std::cmp::Reverse(doc.get_object(*id).and_then(Object::as_stream).map(|s| s.content.len()).unwrap_or(0)));
    let mut before = 0usize;
    let mut after = 0usize;
    let all_images = image_bytes(doc);
    let max_side_pt = max_page_side(doc);
    for id in ids.iter().take(6) {
        let Some(job) = job(doc, *id, 250_000_000) else { continue };
        let old = job.old_len;
        before += old;
        after += shrink(&job, s, max_side_pt / 72.0 * s.dpi).map(|r| r.data.len()).unwrap_or(old);
    }
    if before == 0 || all_images == 0 {
        return 1.0;
    }
    // Images that are not candidates (masks, CMYK…) keep their size.
    let cand_total: usize = ids.iter().filter_map(|id| doc.get_object(*id).and_then(Object::as_stream).ok()).map(|s| s.content.len()).sum();
    let r = after as f64 / before as f64;
    (cand_total as f64 * r + (all_images - cand_total.min(all_images)) as f64) / all_images as f64
}

/// The longest page side in points (A4 when no page says): image resolutions are
/// measured against it.
fn max_page_side(doc: &Document) -> f64 {
    let m = doc.get_pages().values().filter_map(|p| pdf::page_box(doc, *p)).map(|(w, h)| w.max(h)).fold(0.0f64, f64::max);
    if m > 0.0 { m } else { 842.0 }
}

/// The file's size once rewritten, images left as they are (object streams, unused
/// objects removed): what the restructuring alone gains. None for very large files.
fn restructured_size(doc: &Document, in_bytes: u64) -> Option<(f64, f64)> {
    if in_bytes > 60_000_000 {
        return None;
    }
    let mut d = doc.clone();
    d.prune_objects();
    d.delete_zero_length_streams();
    d.compress();
    let mut buf = Vec::new();
    let opts = lopdf::SaveOptions::builder().use_object_streams(true).use_xref_streams(true).compression_level(9).build();
    d.save_with_options(&mut buf, opts).ok()?;
    Some((buf.len() as f64, image_bytes(&d) as f64))
}

fn image_bytes(doc: &Document) -> usize {
    doc.objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice()))
        .map(|s| s.content.len())
        .sum()
}

/// Text and drawing colours to gray: `rg`/`RG` (RGB) and `k`/`K` (CMYK) become
/// `g`/`G`, in page contents and form XObjects. Colours set through other colour
/// spaces (`sc`, `scn`) stay as they are.
fn gray_vectors(doc: &mut Document) {
    let mut ids: Vec<ObjectId> = Vec::new();
    for page in doc.get_pages().values() {
        ids.extend(doc.get_page_contents(*page));
    }
    for (id, o) in &doc.objects {
        if let Ok(st) = o.as_stream() {
            if st.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Form".as_slice()) {
                ids.push(*id);
            }
        }
    }
    ids.sort();
    ids.dedup();
    for id in ids {
        let plain = match doc.get_object(id).and_then(Object::as_stream) {
            Ok(st) => st.decompressed_content().or_else(|_| Ok::<_, lopdf::Error>(st.content.clone())).ok(),
            Err(_) => None,
        };
        let Some(plain) = plain else { continue };
        if let Some(new) = gray_ops(&plain) {
            if let Ok(Object::Stream(st)) = doc.get_object_mut(id) {
                st.set_plain_content(new);
                let _ = st.compress();
            }
        }
    }
}

/// Rewrites the colour operators of one content stream in place, byte by byte: every
/// other byte is copied as it is. (Parsing and re-encoding the stream would lose
/// operands that a producer split across two content streams of the same page.)
fn gray_ops(data: &[u8]) -> Option<Vec<u8>> {
    let white = |c: u8| matches!(c, 0 | 9 | 10 | 12 | 13 | 32);
    let delim = |c: u8| matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%');
    let mut out = Vec::with_capacity(data.len());
    // Operands since the last operator: Some((value, where it starts in `out`)) for numbers.
    let mut operands: Vec<Option<(f32, usize)>> = Vec::new();
    let mut changed = false;
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        let start = i;
        if white(c) {
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            b'%' => {
                while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                let mut depth = 0;
                while i < data.len() {
                    match data[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                operands.push(None);
            }
            b'<' if data.get(i + 1) == Some(&b'<') => {
                i += 2;
                operands.push(None);
            }
            b'<' => {
                while i < data.len() && data[i] != b'>' {
                    i += 1;
                }
                i += 1;
                operands.push(None);
            }
            b'>' | b'[' | b']' | b'{' | b'}' | b')' => {
                i += 1;
                operands.push(None);
            }
            b'/' => {
                i += 1;
                while i < data.len() && !white(data[i]) && !delim(data[i]) {
                    i += 1;
                }
                operands.push(None);
            }
            _ => {
                while i < data.len() && !white(data[i]) && !delim(data[i]) {
                    i += 1;
                }
                let tok = &data[start..i];
                if let Some(v) = std::str::from_utf8(tok).ok().and_then(|t| t.parse::<f32>().ok()).filter(|_| tok[0] != b'+' || tok.len() > 1) {
                    operands.push(Some((v, out.len())));
                } else if tok == b"BI" {
                    // Inline image: copy through to "EI", the data is binary.
                    let mut j = i;
                    while j + 1 < data.len() && !(data[j - 1] == b'I' && data[j] == b'D' && white(data[j - 2]) && white(data[j + 1])) {
                        j += 1;
                    }
                    j += 2;
                    while j + 2 < data.len() && !(white(data[j]) && data[j + 1] == b'E' && data[j + 2] == b'I' && data.get(j + 3).is_none_or(|&x| white(x))) {
                        j += 1;
                    }
                    i = (j + 3).min(data.len());
                    operands.clear();
                } else {
                    let n = match tok {
                        b"rg" | b"RG" => 3,
                        b"k" | b"K" => 4,
                        _ => 0,
                    };
                    let nums: Vec<(f32, usize)> = operands.iter().flatten().copied().collect();
                    if n > 0 && operands.len() == n && nums.len() == n {
                        let v: Vec<f32> = nums.iter().map(|x| x.0).collect();
                        let g = if n == 3 { 0.299 * v[0] + 0.587 * v[1] + 0.114 * v[2] } else { 1.0 - (0.299 * v[0] + 0.587 * v[1] + 0.114 * v[2] + v[3]).min(1.0) };
                        out.truncate(nums[0].1);
                        out.extend_from_slice(format!("{:.4} {}", g.clamp(0.0, 1.0), if tok[0].is_ascii_uppercase() { "G" } else { "g" }).as_bytes());
                        changed = true;
                        operands.clear();
                        continue;
                    }
                    operands.clear();
                }
            }
        }
        out.extend_from_slice(&data[start..i.min(data.len())]);
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::gray_ops;

    fn run(s: &str) -> String {
        String::from_utf8(gray_ops(s.as_bytes()).unwrap_or_else(|| s.as_bytes().to_vec())).unwrap()
    }

    #[test]
    fn colours_become_gray_and_nothing_else_moves() {
        assert_eq!(run("q 1 0 0 rg 0 0 1 RG BT (Hi) Tj ET Q"), "q 0.2990 g 0.1140 G BT (Hi) Tj ET Q");
        assert_eq!(run("0 0 0 1 k\n.5 .5 .5 0 K"), "0.0000 g\n0.5000 G");
        // In strings, after names or with the wrong operand count: untouched.
        assert_eq!(run("BT (1 0 0 rg) Tj ET /CS0 cs 1 0 0 sc"), "BT (1 0 0 rg) Tj ET /CS0 cs 1 0 0 sc");
        assert_eq!(run("/P <</MCID 3>> BDC 1 0 rg EMC"), "/P <</MCID 3>> BDC 1 0 rg EMC");
        // Operands left at the end (the operator is in the next stream) stay.
        assert_eq!(run("1 0 0 rg [(cette) -3 (transmission)]"), "0.2990 g [(cette) -3 (transmission)]");
        // An inline image's bytes are never read as operators.
        let s = "BI /W 1 /H 1 /BPC 8 /CS /RGB ID \u{1} 1 0 0 rg\u{2} EI 0 1 0 rg";
        assert_eq!(run(s), "BI /W 1 /H 1 /BPC 8 /CS /RGB ID \u{1} 1 0 0 rg\u{2} EI 0.5870 g");
        // Escaped parentheses inside strings.
        assert_eq!(run(r"(a\) 1 0 0 rg) Tj 1 1 1 rg"), r"(a\) 1 0 0 rg) Tj 1.0000 g");
    }
}
