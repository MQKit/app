//! Tool 9: HEIC to JPG, through the Windows HEIF codec (WIC). On PCs without the
//! HEIF/HEVC extensions, through FFmpeg (downloaded on demand, see `heic_ffmpeg`).

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img::{self, Kind};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct HeicToJpg;

impl Plugin for HeicToJpg {
    fn id(&self) -> &'static str {
        "heic-to-jpg"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("HEIC în JPG", "HEIC to JPG"),
            "summary": t("Poze de pe iPhone", "iPhone photos"),
            "keywords": { "ro": ["iphone", "heif", "poze", "apple", "telefon"], "en": ["iphone", "heif", "photos", "apple", "phone"] },
            "category": "image",
            "icon": "device-mobile",
            "input": { "extensions": ["heic", "heif"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["jpg", "png"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["width", "height", "imageCount", "hasExif", "heifCodec"] },
            "requires": [ { "engine": "ffmpeg", "when": { "file": "heifCodec", "eq": "missing", "quant": "any" } } ],
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 2.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "jpg", "display": "chips",
                  "options": [ opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 90,
                  "showIf": { "field": "format", "eq": "jpg" } },
                { "key": "keepMetadata", "type": "toggle", "default": true,
                  "label": t("Păstrează data, camera și locația", "Keep date, camera and location"),
                  "showIf": { "field": "format", "eq": "jpg" } },
                { "key": "allImages", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Toate imaginile din fișier (rafale)", "All images in the file (bursts)"),
                  "showIf": { "file": "imageCount", "gt": 1 } }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "heif_sequence": t("Fișierul e o mică animație HEIF, nu o poză.", "This file is a short HEIF animation, not a photo.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        if img::sniff(f.input_path()) != Kind::Heic {
            return Analysis::status("unsupported", codes::UNSUPPORTED_FILE);
        }
        match img::analyze(f.input_path()) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == "heif_sequence" => Analysis::status("unsupported", "heif_sequence"),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        let count = if ctx.req.bool_or("allImages", false) {
            img::heic_count(path).unwrap_or(1).max(1)
        } else {
            1
        };
        let format = ctx.req.str_or("format", "jpg");
        let q = ctx.req.f64_or("quality", 90.0).round() as u8;
        let keep = ctx.req.bool_or("keepMetadata", true);
        let dir = ctx.file_dir(&f.id)?;
        for i in 0..count {
            ctx.check_cancel()?;
            let base = i as f64 / count as f64;
            let span = 1.0 / count as f64;
            ctx.progress(&f.id, base, "decode", 1, 2);
            let mut decoding = |x: f64| {
                ctx.progress(&f.id, base + span * 0.4 * x, "decode", 1, 2);
                !ctx.cancel.is_cancelled()
            };
            let loaded = img::load_heic(path, i, ctx.req, &mut decoding)?;
            ctx.check_cancel()?;
            ctx.progress(&f.id, base + span * 0.4, "encode", 2, 2);
            let report = |x: f64| ctx.progress(&f.id, base + span * (0.4 + 0.55 * x), "encode", 2, 2);
            let data = if format == "png" {
                // Phone photos are big: the optimizer would take many seconds for a few percent.
                img::encode_png_quick(&loaded.img)?
            } else {
                img::encode_jpeg_p(&loaded.img, q, if keep { loaded.exif.as_deref() } else { None }, loaded.icc.as_deref(), [255, 255, 255], &report)?
            };
            let ext = if format == "png" { "png" } else { "jpg" };
            let out = dir.join(format!("result-{}.{ext}", i + 1));
            write_file(&out, &data)?;
            let vars = if count > 1 { json!({"n": i + 1}) } else { json!({}) };
            ctx.emit_output(&f.id, &out, ext, vars, if i == 0 { in_bytes } else { 0 });
        }
        ctx.progress(&f.id, 1.0, "encode", 2, 2);
        Ok(Value::Null)
    }
}
