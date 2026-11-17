//! Tool 7: compress images (JPG with mozjpeg, PNG with imagequant + oxipng, WebP).

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img::{self, Kind};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct ImageCompress;

impl Plugin for ImageCompress {
    fn id(&self) -> &'static str {
        "image-compress"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Comprimare imagini", "Compress images"),
            "summary": t("JPG, PNG, WebP", "JPG, PNG, WebP"),
            "keywords": { "ro": ["micșorează", "poze", "fotografii", "reduce", "optimizează"], "en": ["shrink", "photos", "reduce", "optimize", "smaller"] },
            "category": "image",
            "icon": "photo-down",
            "input": { "extensions": ["jpg", "jpeg", "jfif", "png", "webp"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["jpg", "png", "webp"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["format", "width", "height", "hasAlpha", "hasExif"] },
            "ui": { "preview": "beforeAfter", "estimate": true, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [
                { "id": "balanced", "label": t("Echilibrat", "Balanced"), "default": true,
                  "values": { "mode": "quality", "quality": 80, "maxDimension": 0, "pngLossy": true } },
                { "id": "web", "label": t("Pentru web", "For the web"),
                  "values": { "mode": "quality", "quality": 75, "maxDimension": 2560, "pngLossy": true } },
                { "id": "smallest", "label": t("Cât mai mic", "Smallest"),
                  "values": { "mode": "quality", "quality": 60, "maxDimension": 1920, "pngLossy": true } },
                { "id": "best", "label": t("Calitate maximă", "Best quality"),
                  "values": { "mode": "quality", "quality": 90, "maxDimension": 0, "pngLossy": false } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "mode", "type": "select", "label": t("Cum comprimăm", "How to compress"), "default": "quality",
                  "options": [ opt("quality", "După calitate", "By quality"), opt("targetSize", "Sub o mărime", "Under a size") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 80,
                  "showIf": { "field": "mode", "eq": "quality" } },
                { "key": "targetSize", "type": "number", "label": t("Mărime maximă per imagine", "Maximum size per image"),
                  "min": 10000, "max": 100000000, "default": 500000,
                  "units": { "base": "B", "display": [ { "id": "KB", "factor": 1000 }, { "id": "MB", "factor": 1000000 } ], "defaultDisplay": "KB" },
                  "showIf": { "field": "mode", "eq": "targetSize" } },
                { "key": "maxDimension", "type": "select", "label": t("Latura cea mai lungă", "Longest side"), "default": 0,
                  "options": [ opt(0, "Neschimbată", "Unchanged"), opt(3840, "3840 px (4K)", "3840 px (4K)"), opt(2560, "2560 px", "2560 px"),
                               opt(1920, "1920 px (Full HD)", "1920 px (Full HD)"), opt(1280, "1280 px", "1280 px"), opt(800, "800 px", "800 px") ],
                  "allowCustom": { "type": "number", "min": 16, "max": 20000, "unit": "px" } },
                { "key": "pngLossy", "type": "toggle", "advanced": true, "group": "more", "default": true,
                  "label": t("PNG cu paletă de culori (mult mai mic)", "PNG with a color palette (much smaller)"),
                  "help": t("Reduce PNG-urile la cel mult 256 de culori, cu diferențe greu de văzut. Oprit: PNG-ul scade puțin, dar rămâne identic.",
                            "Reduces PNGs to at most 256 colors, with differences hard to see. Off: the PNG shrinks a little but stays identical.") },
                { "key": "keepMetadata", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Păstrează data, camera și locația (JPG, WebP)", "Keep date, camera and location (JPG, WebP)"),
                  "help": t("Oprit: informațiile EXIF, inclusiv locația GPS, sunt șterse.", "Off: EXIF data, including GPS location, is removed.") },
                { "key": "ifLarger", "type": "select", "advanced": true, "group": "more", "default": "keepOriginal",
                  "label": t("Dacă iese mai mare", "If the result is larger"),
                  "options": [ opt("keepOriginal", "Nu scrie nimic", "Don't write anything"), opt("writeAnyway", "Scrie oricum", "Write anyway") ] }
            ],
            "rules": [
                { "id": "png_lossless_small", "severity": "info",
                  "when": { "all": [ { "file": "format", "eq": "png" }, { "field": "pngLossy", "eq": false } ] },
                  "message": t("Fără paletă de culori, PNG-urile scad de obicei cu doar 5–20%.", "Without a color palette, PNGs usually shrink by only 5–20%.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_comprimat", "{name}_compressed") } },
            "errors": {}
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        match img::analyze(f.input_path()) {
            Ok(p) => {
                let fmt = p["format"].as_str().unwrap_or("");
                if !matches!(fmt, "jpeg" | "png" | "webp") {
                    return Analysis::status("unsupported", codes::UNSUPPORTED_FILE);
                }
                Analysis::ok(p)
            }
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        let Some(f) = super::preview_file(req) else { return Ok(None) };
        let loaded = img::load(f.input_path(), req)?;
        let image = img::fit_long_side(loaded.img, req.f64_or("maxDimension", 0.0) as u32);
        let before = crate::common::preview::crop(&image, req);
        let q = if req.str_or("mode", "quality") == "targetSize" { 70 } else { req.f64_or("quality", 80.0).round() as u8 };
        let (data, ext) = encode_kind(loaded.kind, &before, q, req.bool_or("pngLossy", true))?;
        crate::common::preview::write(req, &before, &data, ext).map(Some)
    }

    /// Size estimate: the first file encoded for real, the ratio applied to the rest.
    fn estimate(&self, req: &Request) -> Option<Value> {
        let total: u64 = req.files.iter().map(|f| file_size(f.input_path())).sum();
        let f = req.files.first()?;
        let in_bytes = file_size(f.input_path()).max(1);
        if req.str_or("mode", "quality") == "targetSize" {
            let t = req.f64_or("targetSize", 500_000.0);
            let out: f64 = req.files.iter().map(|f| (file_size(f.input_path()) as f64).min(t)).sum();
            return Some(json!({ "inputBytes": total, "outputBytes": out.round(), "confidence": "medium", "method": "formula" }));
        }
        let loaded = img::load(f.input_path(), req).ok()?;
        let image = img::fit_long_side(loaded.img, req.f64_or("maxDimension", 0.0) as u32);
        let q = req.f64_or("quality", 80.0).round() as u8;
        let (data, _) = encode_kind(loaded.kind, &image, q, req.bool_or("pngLossy", true)).ok()?;
        let ratio = (data.len() as f64 / in_bytes as f64).min(1.0);
        let confidence = if req.files.len() == 1 { "high" } else { "medium" };
        Some(json!({ "inputBytes": total, "outputBytes": (total as f64 * ratio).round(), "confidence": confidence, "method": "sample" }))
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "decode", 1, 3);
        let loaded = img::load(path, ctx.req)?;
        ctx.progress(&f.id, 0.25, "resize", 2, 3);
        let max_dim = ctx.req.f64_or("maxDimension", 0.0) as u32;
        let image = img::fit_long_side(loaded.img, max_dim);
        let keep = ctx.req.bool_or("keepMetadata", false);
        let png_lossy = ctx.req.bool_or("pngLossy", true);
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.35, "encode", 3, 3);
        // Encoding is 0.35 → 0.95; in "target size" mode each attempt takes a slice.
        let target_mode = ctx.req.str_or("mode", "quality") == "targetSize";
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let slices = if target_mode { 8.0 } else { 1.0 };
        let report = |frac: f64| {
            let a = attempt.load(std::sync::atomic::Ordering::Relaxed) as f64;
            ctx.progress(&f.id, 0.35 + 0.6 * ((a + frac) / slices).min(1.0), "encode", 3, 3);
        };
        let next = || {
            attempt.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            report(0.0);
        };

        let exif = if keep { loaded.exif.as_deref() } else { None };
        let icc = loaded.icc.as_deref();
        let animated_png = if loaded.kind == Kind::Png {
            std::fs::read(path).ok().filter(|d| img::png_is_animated(d))
        } else {
            None
        };
        let frames: Option<Vec<(image::RgbaImage, u32)>> = if loaded.kind == Kind::Webp {
            img::animation_frames(path).map(|fs| {
                fs.into_iter()
                    .map(|(f, d)| match max_dim {
                        0 => (f, d),
                        m => (img::fit_long_side(image::DynamicImage::ImageRgba8(f), m).to_rgba8(), d),
                    })
                    .collect()
            })
        } else {
            None
        };
        let (ext, encode): (&str, Box<dyn Fn(u8) -> Result<Vec<u8>, FileError>>) = match loaded.kind {
            Kind::Jpeg => ("jpg", Box::new(|q| {
                let r = img::encode_jpeg_p(&image, q, exif, icc, [255, 255, 255], &report);
                next();
                r
            })),
            // An animated PNG keeps its frames: only a lossless rewrite of the file itself.
            Kind::Png if animated_png.is_some() => ("png", Box::new(|_q| {
                let mut opts = oxipng::Options::from_preset(2);
                opts.strip = oxipng::StripChunks::Safe;
                let r = oxipng::optimize_from_memory(animated_png.as_deref().unwrap_or_default(), &opts).map_err(|e| FileError::engine(e.to_string()));
                next();
                r
            })),
            Kind::Png => ("png", Box::new(|q| {
                let r = img::encode_png(&image, png_lossy, q).map(|d| match icc {
                    Some(p) => img::png_with_icc(d, p),
                    None => d,
                });
                next();
                r
            })),
            // An animated WebP stays animated: every frame recompressed.
            Kind::Webp if frames.is_some() => ("webp", Box::new(|q| {
                let r = img::encode_webp_anim(frames.as_deref().unwrap_or_default(), q, false);
                next();
                r
            })),
            Kind::Webp => ("webp", Box::new(|q| {
                let r = img::encode_webp(&image, q, false)
                    .map(|d| img::webp_with_meta(d, exif, icc, image.width(), image.height(), image.color().has_alpha()));
                next();
                r
            })),
            _ => return Err(FileError::new(codes::UNSUPPORTED_FILE)),
        };

        let (data, q) = if target_mode && !(loaded.kind == Kind::Png && (!png_lossy || animated_png.is_some())) {
            let target = ctx.req.f64_or("targetSize", 500_000.0) as u64;
            let (d, q, fits) = img::fit_quality(target, 5, 95, &encode)?;
            if !fits {
                return Err(FileError::with(codes::TARGET_UNREACHABLE, json!({"bestBytes": d.len()}))
                    .suggest(json!({"mode": "quality", "quality": 50, "maxDimension": 1920})));
            }
            (d, q)
        } else {
            let q = ctx.req.f64_or("quality", 80.0).round() as u8;
            let d = encode(q)?;
            // Lossless PNG can't trade quality for size: over the target, say so.
            let target = ctx.req.f64_or("targetSize", 500_000.0) as usize;
            if target_mode && d.len() > target {
                return Err(FileError::with(codes::TARGET_UNREACHABLE, json!({"bestBytes": d.len()}))
                    .suggest(json!({"mode": "quality", "pngLossy": true, "maxDimension": 1920})));
            }
            (d, q)
        };
        ctx.check_cancel()?;

        if data.len() as u64 >= in_bytes && ctx.req.str_or("ifLarger", "keepOriginal") == "keepOriginal" && max_dim == 0 {
            return Err(FileError::skipped(codes::OUTPUT_LARGER, json!({"inputBytes": in_bytes, "outputBytes": data.len()})));
        }
        let out = ctx.file_dir(&f.id)?.join(format!("result.{ext}"));
        write_file(&out, &data)?;
        ctx.progress(&f.id, 1.0, "encode", 3, 3);
        ctx.emit_output(&f.id, &out, ext, json!({"width": image.width(), "height": image.height()}), in_bytes);
        Ok(json!({ "quality": q }))
    }
}

fn encode_kind(kind: Kind, image: &image::DynamicImage, q: u8, png_lossy: bool) -> Result<(Vec<u8>, &'static str), FileError> {
    Ok(match kind {
        Kind::Jpeg => (img::encode_jpeg(image, q, None, None, [255, 255, 255])?, "jpg"),
        Kind::Png => (img::encode_png(image, png_lossy, q)?, "png"),
        Kind::Webp => (img::encode_webp(image, q, false)?, "webp"),
        _ => return Err(FileError::new(codes::UNSUPPORTED_FILE)),
    })
}
