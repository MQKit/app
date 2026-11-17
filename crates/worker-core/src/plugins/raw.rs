//! RAW photos: camera raw files (Canon, Nikon, Sony, Fujifilm, Olympus, Panasonic,
//! Pentax, DNG…) to JPG, PNG, TIFF or WebP, with rawler (LGPL-2.1, from dnglab).
//! "As the camera made it" takes the full-size picture every camera stores inside its
//! raw file (its colours and style); "developed" builds the picture from the sensor data.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img;
use contract::{opt, t, FileError, FileRef, Request};
use image::DynamicImage;
use rawler::decoders::{Orientation, RawDecodeParams};
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;
use serde_json::{json, Map, Value};
use std::path::Path;

pub const RAW: &[&str] = &["cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "dng", "srw", "3fr", "iiq", "erf", "kdc", "dcr", "mrw", "mef", "mos", "rwl", "x3f"];

pub struct Raw;

impl Plugin for Raw {
    fn id(&self) -> &'static str {
        "raw"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Poze RAW", "RAW photos"),
            "summary": t("Din aparatul foto în JPG", "From the camera to JPG"),
            "keywords": { "ro": ["raw în jpg", "cr2", "cr3", "nef", "arw", "raf", "dng", "poze raw"], "en": ["raw to jpg", "cr2", "cr3", "nef", "arw", "raf", "dng", "raw photos"] },
            "category": "image",
            "icon": "camera",
            "input": { "extensions": RAW, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["jpg", "png", "tif", "webp"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["make", "model", "width", "height"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 1.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "jpg", "display": "chips",
                  "options": [ opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG"), opt("tif", "TIFF 16 biți", "16-bit TIFF"), opt("webp", "WebP", "WebP") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 50, "max": 100, "step": 1, "unit": "%", "default": 92,
                  "showIf": { "field": "format", "in": ["jpg", "webp"] } },
                { "key": "source", "type": "select", "label": t("Poza", "The picture"), "default": "camera",
                  "options": [ opt("camera", "Cum a făcut-o aparatul (culorile lui)", "As the camera made it (its colours)"),
                               opt("develop", "Developată din datele senzorului", "Developed from the sensor data") ],
                  "help": t("Dacă aparatul n-a pus în fișier o poză la mărime întreagă, poza se developează.",
                            "If the camera didn't store a full-size picture in the file, the picture is developed.") }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "raw_unsupported": t("Acest aparat foto nu e încă cunoscut: {message}", "This camera isn't supported yet: {message}")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let mut p = Map::new();
        if let Ok(src) = RawSource::new(f.input_path()) {
            if let Ok(dec) = rawler::get_decoder(&src) {
                if let Ok(md) = dec.raw_metadata(&src, &RawDecodeParams::default()) {
                    p.insert("make".into(), json!(md.make));
                    p.insert("model".into(), json!(md.model));
                }
                return Analysis::ok(p);
            }
        }
        Analysis::status("unsupported", "raw_unsupported")
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let im = picture(ctx, &f.id, path, req.str_or("source", "camera") == "camera")?;
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.8, "save", 2, 2);
        let format = req.str_or("format", "jpg");
        let q = req.f64_or("quality", 92.0).clamp(50.0, 100.0) as u8;
        let bytes = match format {
            "png" => img::encode_png_quick(&DynamicImage::ImageRgb8(im.to_rgb8()))?,
            "webp" => img::encode_webp(&DynamicImage::ImageRgb8(im.to_rgb8()), q, false)?,
            "tif" => {
                let mut buf = Vec::new();
                DynamicImage::ImageRgb16(im.to_rgb16()).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Tiff).map_err(img::img_err)?;
                buf
            }
            _ => img::encode_jpeg(&DynamicImage::ImageRgb8(im.to_rgb8()), q, None, None, [255, 255, 255])?,
        };
        let dest = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        write_file(&dest, &bytes)?;
        ctx.progress(&f.id, 1.0, "save", 2, 2);
        ctx.emit_output(&f.id, &dest, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn unsupported(e: impl std::fmt::Display) -> FileError {
    FileError::with("raw_unsupported", json!({ "message": e.to_string() }))
}

fn orient(im: DynamicImage, o: Orientation) -> DynamicImage {
    match o {
        Orientation::HorizontalFlip => im.fliph(),
        Orientation::Rotate180 => im.rotate180(),
        Orientation::VerticalFlip => im.flipv(),
        Orientation::Transpose => im.rotate90().fliph(),
        Orientation::Rotate90 => im.rotate90(),
        Orientation::Transverse => im.rotate270().fliph(),
        Orientation::Rotate270 => im.rotate270(),
        _ => im,
    }
}

/// The camera's own full-size picture when asked for and there is one, else developed.
fn picture(ctx: &Ctx, id: &str, path: &Path, camera: bool) -> Result<DynamicImage, FileError> {
    let src = RawSource::new(path).map_err(super::io_err)?;
    let dec = rawler::get_decoder(&src).map_err(unsupported)?;
    let params = RawDecodeParams::default();
    let raw = dec.raw_image(&src, &params, false).map_err(unsupported)?;
    ctx.check_cancel()?;
    // Only rawler's DNG and CR3 decoders fill in the orientation: for the others (NEF, ARW,
    // CR2, ORF, RW2…) it comes from the EXIF, or a portrait photo came out on its side.
    let orientation = match raw.orientation {
        Orientation::Unknown | Orientation::Normal => dec.raw_metadata(&src, &params).ok().and_then(|m| m.exif.orientation).map(Orientation::from_u16).unwrap_or(Orientation::Normal),
        o => o,
    };
    if camera {
        // Big enough to stand for the photo: at least 3/4 of the sensor's width.
        let embedded = dec.full_image(&src, &params).ok().flatten().or_else(|| dec.preview_image(&src, &params).ok().flatten());
        if let Some(im) = embedded.filter(|im| im.width().max(im.height()) * 4 >= raw.width.max(raw.height) as u32 * 3) {
            return Ok(orient(im, orientation));
        }
    }
    ctx.progress(id, 0.2, "develop", 1, 2);
    let developed = RawDevelop::default().develop_intermediate(&raw).map_err(unsupported)?.to_dynamic_image().ok_or_else(|| unsupported("develop"))?;
    Ok(orient(developed, orientation))
}
