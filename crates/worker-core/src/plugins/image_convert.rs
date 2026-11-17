//! Tool 8: convert images between formats.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img;
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::ImageFormat;
use serde_json::{json, Value};

pub struct ImageConvert;

pub const IMAGE_INPUTS: &[&str] = &["jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff", "ico", "heic", "heif", "avif"];

impl Plugin for ImageConvert {
    fn id(&self) -> &'static str {
        "image-convert"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Conversie imagini", "Convert images"),
            "summary": t("Schimbă formatul", "Change the format"),
            "keywords": { "ro": ["format", "jpg", "png", "webp", "transformă", "bmp", "tiff", "ico"], "en": ["format", "jpg", "png", "webp", "convert", "bmp", "tiff", "ico"] },
            "category": "image",
            "icon": "arrows-exchange",
            "requires": [ { "engine": "ffmpeg", "when": { "file": "heifCodec", "eq": "missing", "quant": "any" } } ],
            "input": { "extensions": IMAGE_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["jpg", "png", "webp", "gif", "bmp", "tiff", "ico"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "width", "height", "hasAlpha", "isAnimated"] },
            "ui": { "preview": "beforeAfter", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 2.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "jpg", "display": "chips",
                  "options": [ opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG"), opt("webp", "WebP", "WebP"),
                               opt("gif", "GIF", "GIF"), opt("bmp", "BMP", "BMP"), opt("tiff", "TIFF", "TIFF"), opt("ico", "ICO", "ICO") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 90,
                  "showIf": { "all": [ { "field": "format", "in": ["jpg", "webp"] }, { "not": { "field": "webpLossless", "eq": true } } ] } },
                { "key": "background", "type": "color", "label": t("Fundal în locul transparenței", "Background instead of transparency"), "default": "#ffffff",
                  "showIf": { "all": [ { "field": "format", "in": ["jpg", "bmp"] }, { "file": "hasAlpha", "eq": true } ] } },
                { "key": "webpLossless", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("WebP fără pierderi", "Lossless WebP"), "showIf": { "field": "format", "eq": "webp" } },
                { "key": "keepMetadata", "type": "toggle", "advanced": true, "group": "more", "default": true,
                  "label": t("Păstrează data, camera și locația", "Keep date, camera and location"),
                  "help": t("Se păstrează la JPG și WebP.", "Kept for JPG and WebP."), "showIf": { "field": "format", "in": ["jpg", "webp"] } }
            ],
            "rules": [
                { "id": "animation_lost", "severity": "warning",
                  "when": { "all": [ { "file": "isAnimated", "eq": true }, { "field": "format", "notIn": ["gif", "webp"] } ] },
                  "message": t("Formatul ales nu are animație: imaginile animate păstrează doar primul cadru. Alege GIF sau WebP ca să rămână animate.",
                               "This format has no animation: animated images keep only the first frame. Pick GIF or WebP to keep them animated.") },
                { "id": "pages_lost", "severity": "warning",
                  "when": { "all": [ { "file": "format", "eq": "tiff" }, { "file": "imageCount", "gt": 1 } ] },
                  "message": t("Un TIFF cu mai multe pagini se transformă doar cu prima pagină. Pentru toate paginile, folosește „Imagini în PDF”.",
                               "A multi-page TIFF is converted with its first page only. For every page, use “Images to PDF”.") },
                { "id": "ico_resized", "severity": "info", "when": { "field": "format", "eq": "ico" },
                  "message": t("Iconițele au cel mult 256 × 256 px; imaginile mai mari se micșorează.", "Icons are at most 256 × 256 px; larger images are scaled down.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {}
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        match img::analyze(f.input_path()) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        let Some(f) = super::preview_file(req) else { return Ok(None) };
        let loaded = img::load(f.input_path(), req)?;
        let before = crate::common::preview::crop(&loaded.img, req);
        let format = req.str_or("format", "jpg");
        let q = req.f64_or("quality", 90.0).round() as u8;
        let bg = img::parse_color(req.str_or("background", "#ffffff"));
        let data = match format {
            "jpg" => img::encode_jpeg(&before, q, None, None, bg)?,
            "webp" => img::encode_webp(&before, q, req.bool_or("webpLossless", false))?,
            "bmp" => img::encode_with(&before, ImageFormat::Bmp, bg)?,
            "gif" => img::encode_with(&before, ImageFormat::Gif, bg)?,
            // PNG, TIFF, ICO are lossless: the preview is the original.
            _ => img::encode_png(&before, false, 100)?,
        };
        crate::common::preview::write(req, &before, &data, format).map(Some)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "decode", 1, 2);
        let format = ctx.req.str_or("format", "jpg");
        // Animated GIF/WebP to GIF or WebP: every frame is kept.
        if matches!(format, "gif" | "webp") && f.analysis.get("isAnimated").and_then(Value::as_bool) == Some(true) {
            if let Some(frames) = img::animation_frames(path) {
                ctx.progress(&f.id, 0.4, "encode", 2, 2);
                let q = ctx.req.f64_or("quality", 90.0) as u8;
                let data = if format == "gif" {
                    img::encode_gif_anim(&frames)?
                } else {
                    img::encode_webp_anim(&frames, q, ctx.req.bool_or("webpLossless", false))?
                };
                let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
                write_file(&out, &data)?;
                ctx.progress(&f.id, 1.0, "encode", 2, 2);
                ctx.emit_output(&f.id, &out, format, json!({ "frames": frames.len() }), in_bytes);
                return Ok(Value::Null);
            }
        }
        let loaded = img::load(path, ctx.req)?;
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.4, "encode", 2, 2);
        let report = |x: f64| ctx.progress(&f.id, 0.4 + 0.55 * x, "encode", 2, 2);
        let q = ctx.req.f64_or("quality", 90.0).round() as u8;
        let bg = img::parse_color(ctx.req.str_or("background", "#ffffff"));
        let data = match format {
            "jpg" => {
                let exif = if ctx.req.bool_or("keepMetadata", true) { loaded.exif.as_deref() } else { None };
                img::encode_jpeg_p(&loaded.img, q, exif, loaded.icc.as_deref(), bg, &report)?
            }
            "png" => {
                // Big photos: the optimizer would take many seconds for a few percent.
                let big = loaded.img.width() as u64 * loaded.img.height() as u64 > 6_000_000;
                let d = if big { img::encode_png_quick(&loaded.img)? } else { img::encode_png(&loaded.img, false, 100)? };
                match loaded.icc.as_deref() {
                    Some(p) => img::png_with_icc(d, p),
                    None => d,
                }
            }
            "webp" => {
                let d = img::encode_webp(&loaded.img, q, ctx.req.bool_or("webpLossless", false))?;
                let exif = loaded.exif.as_deref().filter(|_| ctx.req.bool_or("keepMetadata", true));
                img::webp_with_meta(d, exif, loaded.icc.as_deref(), loaded.img.width(), loaded.img.height(), loaded.img.color().has_alpha())
            }
            "gif" => img::encode_with(&loaded.img, ImageFormat::Gif, bg)?,
            "bmp" => img::encode_with(&loaded.img, ImageFormat::Bmp, bg)?,
            "tiff" => img::encode_with(&loaded.img, ImageFormat::Tiff, bg)?,
            "ico" => img::encode_with(&loaded.img, ImageFormat::Ico, bg)?,
            other => return Err(FileError::with(codes::INVALID_OPTIONS, json!({"format": other}))),
        };
        if f.analysis.get("isAnimated").and_then(Value::as_bool) == Some(true) && !matches!(format, "gif" | "webp") {
            ctx.out.warning(Some(&f.id), "animation_lost", None);
        }
        if f.analysis.get("imageCount").and_then(Value::as_u64).unwrap_or(1) > 1 && loaded.kind == img::Kind::Tiff {
            ctx.out.warning(Some(&f.id), "pages_lost", None);
        }
        ctx.progress(&f.id, 0.95, "encode", 2, 2);
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        write_file(&out, &data)?;
        ctx.progress(&f.id, 1.0, "encode", 2, 2);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}
