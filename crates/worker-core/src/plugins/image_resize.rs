//! Resize images: fit in a box, fill it and crop, stretch, pad, by the long side,
//! width, height or percent. Small images aren't enlarged unless asked.

use super::image_convert::IMAGE_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img::{self, Kind};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageFormat, Rgba, RgbaImage};
use serde_json::{json, Value};

pub struct ImageResize;

impl Plugin for ImageResize {
    fn id(&self) -> &'static str {
        "image-resize"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Redimensionare imagini", "Resize images"),
            "summary": t("Lățime, înălțime, decupare", "Width, height, crop"),
            "keywords": { "ro": ["micșorează", "mărește", "dimensiune", "pixeli", "decupează"], "en": ["smaller", "bigger", "dimensions", "pixels", "crop"] },
            "category": "image",
            "icon": "resize",
            "requires": [ { "engine": "ffmpeg", "when": { "file": "heifCodec", "eq": "missing", "quant": "any" } } ],
            "input": { "extensions": IMAGE_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["jpg", "png", "webp", "gif", "bmp", "tiff"],
                        "nameVariables": [ { "id": "width", "label": t("lățime", "width") }, { "id": "height", "label": t("înălțime", "height") } ],
                        "replaceOriginalAllowed": true },
            "analysis": { "fast": ["format", "width", "height", "hasAlpha", "isAnimated"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 1.5 },
            "presets": [
                { "id": "email", "label": t("Pentru e-mail", "For email"), "default": true, "values": { "mode": "longEdge", "longEdge": 1600 } },
                { "id": "fullhd", "label": t("Full HD", "Full HD"), "values": { "mode": "fit", "width": 1920, "height": 1080 } },
                { "id": "square", "label": t("Pătrat 1080", "Square 1080"), "values": { "mode": "fill", "width": 1080, "height": 1080, "position": "center" } },
                { "id": "story", "label": t("Poveste 1080×1920", "Story 1080×1920"), "values": { "mode": "fill", "width": 1080, "height": 1920, "position": "center" } },
                { "id": "half", "label": t("Jumătate", "Half"), "values": { "mode": "percent", "percent": 50 } },
                { "id": "thumb", "label": t("Miniatură", "Thumbnail"), "values": { "mode": "fill", "width": 256, "height": 256, "position": "center" } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "mode", "type": "select", "label": t("Cum", "How"), "default": "longEdge",
                  "options": [ opt("longEdge", "Latura cea mai lungă", "Longest side"),
                               opt("fit", "Să încapă într-o mărime", "Fit inside a size"),
                               opt("fill", "Să umple mărimea (decupează)", "Fill the size (crops)"),
                               opt("pad", "Să încapă, cu margini", "Fit, with borders"),
                               opt("exact", "Exact (poate deforma)", "Exact (may stretch)"),
                               opt("width", "Doar lățimea", "Width only"),
                               opt("height", "Doar înălțimea", "Height only"),
                               opt("percent", "În procente", "Percent") ] },
                { "key": "longEdge", "type": "number", "label": t("Latura lungă", "Long side"), "default": 1600, "min": 1, "max": 30000, "unit": "px",
                  "showIf": { "field": "mode", "eq": "longEdge" } },
                { "key": "width", "type": "number", "label": t("Lățime", "Width"), "default": 1920, "min": 1, "max": 30000, "unit": "px",
                  "showIf": { "field": "mode", "in": ["fit", "fill", "exact", "pad", "width"] } },
                { "key": "height", "type": "number", "label": t("Înălțime", "Height"), "default": 1080, "min": 1, "max": 30000, "unit": "px",
                  "showIf": { "field": "mode", "in": ["fit", "fill", "exact", "pad", "height"] } },
                { "key": "percent", "type": "range", "label": t("Mărime", "Size"), "default": 50, "min": 1, "max": 400, "step": 1, "unit": "%",
                  "showIf": { "field": "mode", "eq": "percent" } },
                { "key": "position", "type": "select", "label": t("Ce parte păstrez", "Which part to keep"), "default": "center",
                  "options": [ opt("center", "Mijlocul", "The middle"), opt("top", "Sus", "Top"), opt("bottom", "Jos", "Bottom"),
                               opt("left", "Stânga", "Left"), opt("right", "Dreapta", "Right") ],
                  "showIf": { "field": "mode", "eq": "fill" } },
                { "key": "padColor", "type": "color", "label": t("Culoarea marginilor", "Border color"), "default": "#ffffff",
                  "showIf": { "field": "mode", "eq": "pad" } },
                { "key": "noUpscale", "type": "toggle", "default": true, "label": t("Nu mări imaginile mici", "Don't enlarge small images"),
                  "showIf": { "field": "mode", "notIn": ["exact"] } },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "original", "display": "chips",
                  "options": [ opt("original", "La fel", "Same"), opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG"), opt("webp", "WebP", "WebP") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 85,
                  "showIf": { "field": "format", "ne": "png" } },
                { "key": "filter", "type": "select", "advanced": true, "group": "more", "default": "lanczos3",
                  "label": t("Metodă", "Method"),
                  "options": [ opt("lanczos3", "Clară (recomandat)", "Sharp (recommended)"), opt("catmullRom", "Moale", "Soft"),
                               opt("nearest", "Pixeli (pixel art)", "Pixels (pixel art)") ] },
                { "key": "keepMetadata", "type": "toggle", "advanced": true, "group": "more", "default": true,
                  "label": t("Păstrează data, camera și locația", "Keep date, camera and location"),
                  "help": t("Se păstrează la JPG și WebP.", "Kept for JPG and WebP.") }
            ],
            "rules": [
                { "id": "fill_crops", "severity": "info", "when": { "field": "mode", "eq": "fill" },
                  "message": t("Partea care nu încape se taie.", "The part that doesn't fit is cut off.") },
                { "id": "exact_stretches", "severity": "info", "when": { "field": "mode", "eq": "exact" },
                  "message": t("Dacă forma diferă, imaginea se deformează.", "If the shape differs, the image is stretched.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_{width}x{height}", "{name}_{width}x{height}") } },
            "errors": {
                "resize_invalid_dimensions": t("Mărimea aleasă nu e validă.", "The chosen size isn't valid."),
                "resize_too_big": t("Imaginea ar avea {megapixels} megapixeli, peste limita de {limit}. Alege o mărime mai mică.",
                                    "The image would have {megapixels} megapixels, over the limit of {limit}. Choose a smaller size."),
                "animation_flattened": t("S-a păstrat doar primul cadru al animației.", "Only the first frame of the animation was kept.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        match img::analyze(f.input_path()) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "decode", 1, 3);
        let kind = img::sniff(path);
        let format = match req.str_or("format", "original") {
            "original" => same_format(kind),
            other => other,
        };
        let q = req.f64_or("quality", 85.0).round().clamp(1.0, 100.0) as u8;
        let filter = match req.str_or("filter", "lanczos3") {
            "catmullRom" => FilterType::CatmullRom,
            "nearest" => FilterType::Nearest,
            _ => FilterType::Lanczos3,
        };
        let pad = img::parse_color(req.str_or("padColor", "#ffffff"));

        // Animated GIF/WebP kept as GIF/WebP: every frame resized.
        if matches!(format, "gif" | "webp") && f.analysis.get("isAnimated").and_then(Value::as_bool) == Some(true) {
            if let Some(frames) = img::animation_frames(path) {
                let (w0, h0) = frames.first().map(|(f, _)| f.dimensions()).unwrap_or((1, 1));
                let plan = plan(req, w0, h0)?;
                too_big(req, &plan)?;
                let n = frames.len().max(1);
                let mut out_frames = Vec::with_capacity(frames.len());
                for (i, (fr, delay)) in frames.into_iter().enumerate() {
                    ctx.check_cancel()?;
                    ctx.progress(&f.id, 0.1 + 0.6 * i as f64 / n as f64, "resize", 2, 3);
                    out_frames.push((apply(&DynamicImage::ImageRgba8(fr), &plan, filter, pad, true).to_rgba8(), delay));
                }
                ctx.progress(&f.id, 0.75, "encode", 3, 3);
                let data = if format == "gif" { img::encode_gif_anim(&out_frames)? } else { img::encode_webp_anim(&out_frames, q, false)? };
                return finish(ctx, f, format, &data, plan.out_w, plan.out_h, in_bytes);
            }
        }

        let loaded = img::load(path, req)?;
        ctx.check_cancel()?;
        let (w0, h0) = loaded.img.dimensions();
        let plan = plan(req, w0, h0)?;
        too_big(req, &plan)?;
        ctx.progress(&f.id, 0.3, "resize", 2, 3);
        let keep_alpha = matches!(format, "png" | "webp" | "gif" | "tiff");
        let out = apply(&loaded.img, &plan, filter, pad, keep_alpha);
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.6, "encode", 3, 3);
        let report = |x: f64| ctx.progress(&f.id, 0.6 + 0.35 * x, "encode", 3, 3);
        let keep_meta = req.bool_or("keepMetadata", true);
        let data = match format {
            "jpg" => img::encode_jpeg_p(&out, q, loaded.exif.as_deref().filter(|_| keep_meta), loaded.icc.as_deref(), pad, &report)?,
            "png" => {
                let big = out.width() as u64 * out.height() as u64 > 6_000_000;
                let d = if big { img::encode_png_quick(&out)? } else { img::encode_png(&out, false, 100)? };
                match loaded.icc.as_deref() {
                    Some(p) => img::png_with_icc(d, p),
                    None => d,
                }
            }
            "webp" => {
                let d = img::encode_webp(&out, q, false)?;
                img::webp_with_meta(d, loaded.exif.as_deref().filter(|_| keep_meta), loaded.icc.as_deref(), out.width(), out.height(), out.color().has_alpha())
            }
            "gif" => img::encode_with(&out, ImageFormat::Gif, pad)?,
            "bmp" => img::encode_with(&out, ImageFormat::Bmp, pad)?,
            "tiff" => img::encode_with(&out, ImageFormat::Tiff, pad)?,
            other => return Err(FileError::with(codes::INVALID_OPTIONS, json!({ "format": other }))),
        };
        if f.analysis.get("isAnimated").and_then(Value::as_bool) == Some(true) && !matches!(format, "gif" | "webp") {
            ctx.out.warning(Some(&f.id), "animation_flattened", None);
        }
        finish(ctx, f, format, &data, out.width(), out.height(), in_bytes)
    }
}

/// Bigger than the pixel budget: the image wouldn't fit in memory (or open anywhere).
fn too_big(req: &Request, p: &Plan) -> Result<(), FileError> {
    let limit = req.limits.max_megapixels.max(1) as u64;
    let mp = (p.scaled_w as u64 * p.scaled_h as u64).max(p.out_w as u64 * p.out_h as u64) / 1_000_000;
    if mp > limit {
        return Err(FileError::with("resize_too_big", json!({ "megapixels": mp, "limit": limit })));
    }
    Ok(())
}

fn finish(ctx: &Ctx, f: &FileRef, format: &str, data: &[u8], w: u32, h: u32, in_bytes: u64) -> Result<Value, FileError> {
    let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
    write_file(&out, data)?;
    ctx.progress(&f.id, 1.0, "encode", 3, 3);
    ctx.emit_output(&f.id, &out, format, json!({ "width": w, "height": h }), in_bytes);
    Ok(Value::Null)
}

/// The output format when "same" is chosen: formats we can't write well become JPG or PNG.
fn same_format(kind: Kind) -> &'static str {
    match kind {
        Kind::Png | Kind::Ico => "png",
        Kind::Webp => "webp",
        Kind::Gif => "gif",
        Kind::Bmp => "bmp",
        Kind::Tiff => "tiff",
        _ => "jpg",
    }
}

/// What to do with one image: scale to (scaled_w, scaled_h), then optionally crop
/// a (out_w, out_h) window at (crop_x, crop_y), or place it on an (out_w, out_h) canvas.
struct Plan {
    scaled_w: u32,
    scaled_h: u32,
    out_w: u32,
    out_h: u32,
    crop: Option<(u32, u32)>,
    pad: Option<(u32, u32)>,
}

fn dim(req: &Request, key: &str, default: f64) -> f64 {
    req.f64_or(key, default).round().clamp(1.0, 30000.0)
}

fn plan(req: &Request, w0: u32, h0: u32) -> Result<Plan, FileError> {
    if w0 == 0 || h0 == 0 {
        return Err(FileError::new("resize_invalid_dimensions"));
    }
    let (w, h) = (w0 as f64, h0 as f64);
    let no_up = req.bool_or("noUpscale", true);
    let cap = |s: f64| if no_up { s.min(1.0) } else { s };
    let size = |s: f64| ((w * s).round().max(1.0) as u32, (h * s).round().max(1.0) as u32);
    let simple = |s: f64| {
        let (a, b) = size(s);
        Plan { scaled_w: a, scaled_h: b, out_w: a, out_h: b, crop: None, pad: None }
    };
    let mode = req.str_or("mode", "longEdge");
    Ok(match mode {
        "fit" => simple(cap((dim(req, "width", 1920.0) / w).min(dim(req, "height", 1080.0) / h))),
        "width" => simple(cap(dim(req, "width", 1920.0) / w)),
        "height" => simple(cap(dim(req, "height", 1080.0) / h)),
        "percent" => simple(cap(req.f64_or("percent", 50.0).clamp(1.0, 400.0) / 100.0)),
        "exact" => {
            let (a, b) = (dim(req, "width", 1920.0) as u32, dim(req, "height", 1080.0) as u32);
            Plan { scaled_w: a, scaled_h: b, out_w: a, out_h: b, crop: None, pad: None }
        }
        "fill" => {
            let (bw, bh) = (dim(req, "width", 1080.0), dim(req, "height", 1080.0));
            let s = (bw / w).max(bh / h);
            // Too small to fill the box without enlarging: crop to the shape only.
            let s = if no_up && s > 1.0 { 1.0 } else { s };
            let (sw, sh) = size(s);
            let ratio = bw / bh;
            let (ow, oh) = if s >= (bw / w).max(bh / h) - 1e-9 {
                (bw as u32, bh as u32)
            } else if sw as f64 / sh as f64 > ratio {
                ((sh as f64 * ratio).round() as u32, sh)
            } else {
                (sw, (sw as f64 / ratio).round() as u32)
            };
            let (ow, oh) = (ow.clamp(1, sw), oh.clamp(1, sh));
            let (fx, fy) = match req.str_or("position", "center") {
                "top" => (0.5, 0.0),
                "bottom" => (0.5, 1.0),
                "left" => (0.0, 0.5),
                "right" => (1.0, 0.5),
                _ => (0.5, 0.5),
            };
            let cx = ((sw - ow) as f64 * fx).round() as u32;
            let cy = ((sh - oh) as f64 * fy).round() as u32;
            Plan { scaled_w: sw, scaled_h: sh, out_w: ow, out_h: oh, crop: Some((cx, cy)), pad: None }
        }
        "pad" => {
            let (bw, bh) = (dim(req, "width", 1920.0), dim(req, "height", 1080.0));
            let (sw, sh) = size(cap((bw / w).min(bh / h)));
            let (ow, oh) = (bw as u32, bh as u32);
            Plan { scaled_w: sw, scaled_h: sh, out_w: ow, out_h: oh, crop: None, pad: Some(((ow - sw.min(ow)) / 2, (oh - sh.min(oh)) / 2)) }
        }
        _ => simple(cap(dim(req, "longEdge", 1600.0) / w.max(h))),
    })
}

fn apply(src: &DynamicImage, p: &Plan, filter: FilterType, pad: [u8; 3], alpha: bool) -> DynamicImage {
    let scaled = if (p.scaled_w, p.scaled_h) == src.dimensions() { src.clone() } else { img::resize(src, p.scaled_w, p.scaled_h, filter) };
    if let Some((x, y)) = p.crop {
        return scaled.crop_imm(x, y, p.out_w, p.out_h);
    }
    if let Some((x, y)) = p.pad {
        let fill = Rgba([pad[0], pad[1], pad[2], 255]);
        let mut canvas = RgbaImage::from_pixel(p.out_w, p.out_h, fill);
        image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), x as i64, y as i64);
        let out = DynamicImage::ImageRgba8(canvas);
        return if alpha { out } else { DynamicImage::ImageRgb8(out.to_rgb8()) };
    }
    scaled
}
