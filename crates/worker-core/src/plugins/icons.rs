//! Icons: a Windows .ico with several sizes, a favicon pack for a website, a set of
//! PNGs, or the images inside an existing .ico.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img;
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::{imageops::FilterType, DynamicImage, ExtendedColorType, GenericImageView, Rgba, RgbaImage};
use serde_json::{json, Value};
use std::path::Path;

pub struct Icons;

const INPUTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "ico", "heic", "heif", "avif", "svg", "svgz", "emf", "wmf", "ai"];

impl Plugin for Icons {
    fn id(&self) -> &'static str {
        "icons"
    }

    fn manifest(&self) -> Value {
        let not_extract = json!({ "field": "kind", "ne": "extract" });
        json!({
            "name": t("Iconițe", "Icons"),
            "summary": t("ICO și favicon", "ICO and favicons"),
            "keywords": { "ro": ["iconiță", "ico", "favicon", "site", "pictogramă"], "en": ["icon", "ico", "favicon", "website", "app icon"] },
            "category": "image",
            "icon": "icons",
            "requires": [ { "engine": "ffmpeg", "when": { "file": "heifCodec", "eq": "missing", "quant": "any" } } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "multiPerInput", "extensions": ["ico", "png", "webmanifest", "html"],
                        "nameVariables": [ { "id": "file", "label": t("numele fișierului", "file name") } ],
                        "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "width", "height", "hasAlpha"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [
                { "id": "windows", "label": t("Iconiță Windows", "Windows icon"), "default": true, "values": { "kind": "ico", "sizes": "16, 24, 32, 48, 64, 256" } },
                { "id": "favicon", "label": t("Favicon pentru site", "Website favicon"), "values": { "kind": "favicon" } },
                { "id": "pixel", "label": t("Pixel art", "Pixel art"), "values": { "kind": "ico", "sizes": "16, 32, 48, 256", "filter": "nearest", "fit": "pad" } },
                { "id": "extract", "label": t("Scoate imaginile din ICO", "Images out of an ICO"), "values": { "kind": "extract" } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "kind", "type": "select", "label": t("Ce fac", "What to make"), "default": "ico",
                  "options": [ opt("ico", "Fișier ICO (Windows)", "ICO file (Windows)"), opt("favicon", "Pachet favicon pentru site", "Favicon pack for a website"),
                               opt("png", "PNG-uri la mai multe mărimi", "PNGs in several sizes"), opt("extract", "Scoate imaginile dintr-un ICO", "Images out of an ICO") ] },
                { "key": "sizes", "type": "text", "label": t("Mărimi (px)", "Sizes (px)"), "default": "16, 24, 32, 48, 64, 256",
                  "help": t("Între 1 și 256 la ICO, despărțite prin virgulă.", "From 1 to 256 for ICO, separated by commas."),
                  "showIf": { "field": "kind", "in": ["ico", "png"] } },
                { "key": "fit", "type": "select", "label": t("Dacă imaginea nu e pătrată", "If the image isn't square"), "default": "pad",
                  "options": [ opt("pad", "Încape, cu spațiu gol", "Fit, with empty space"), opt("crop", "Taie marginile", "Cut the edges"),
                               opt("stretch", "Întinde", "Stretch") ],
                  "showIf": not_extract },
                { "key": "padding", "type": "range", "label": t("Spațiu în jur", "Space around"), "min": 0, "max": 40, "step": 1, "unit": "%", "default": 0,
                  "showIf": not_extract },
                { "key": "background", "type": "select", "label": t("Fundal", "Background"), "default": "transparent", "display": "chips",
                  "options": [ opt("transparent", "Transparent", "Transparent"), opt("color", "Culoare", "Color") ],
                  "showIf": not_extract },
                { "key": "backgroundColor", "type": "color", "label": t("Culoarea fundalului", "Background color"), "default": "#ffffff",
                  "showIf": { "all": [ not_extract, { "field": "background", "eq": "color" } ] } },
                { "key": "appName", "type": "text", "advanced": true, "group": "more", "default": "",
                  "label": t("Numele site-ului (în manifest)", "Site name (in the manifest)"), "showIf": { "field": "kind", "eq": "favicon" } },
                { "key": "themeColor", "type": "color", "advanced": true, "group": "more", "default": "#ffffff",
                  "label": t("Culoarea temei (în manifest)", "Theme color (in the manifest)"), "showIf": { "field": "kind", "eq": "favicon" } },
                { "key": "filter", "type": "select", "advanced": true, "group": "more", "default": "lanczos3", "label": t("Metodă", "Method"),
                  "options": [ opt("lanczos3", "Clară (recomandat)", "Sharp (recommended)"), opt("nearest", "Pixeli (pixel art)", "Pixels (pixel art)") ],
                  "showIf": not_extract }
            ],
            "rules": [
                { "id": "small_source", "severity": "warning",
                  "when": { "all": [ not_extract, { "any": [ { "file": "width", "lt": 256 }, { "file": "height", "lt": 256 } ] } ] },
                  "message": t("Imaginea e mai mică de 256 px: mărimile mari vor fi neclare.", "The image is smaller than 256 px: the large sizes will be blurry.") },
                { "id": "extract_needs_ico", "severity": "warning",
                  "when": { "all": [ { "field": "kind", "eq": "extract" }, { "file": "format", "ne": "ico" },
                                     { "not": { "file": "format", "ne": "ico", "quant": "all" } } ] },
                  "message": t("Scoaterea imaginilor merge doar cu fișiere ICO.", "Taking the images out only works with ICO files.") },
                // No ICO at all: every file would fail, so the button waits.
                { "id": "extract_no_ico", "severity": "error",
                  "when": { "all": [ { "field": "kind", "eq": "extract" }, { "file": "format", "ne": "ico", "quant": "all" } ] },
                  "message": t("Adaugă un fișier ICO: doar din el se pot scoate imaginile.", "Add an ICO file: the images can only be taken out of one.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{file}", "{file}"), "outputDir": "subfolder", "subfolderName": "{name}_icons" } },
            "errors": {
                "ico_no_sizes": t("Scrie cel puțin o mărime, de exemplu 16, 32, 256.", "Type at least one size, for example 16, 32, 256."),
                "ico_size_out_of_range": t("La ICO, mărimile sunt între 1 și 256 px.", "ICO sizes are from 1 to 256 px."),
                "not_ico": t("Fișierul nu e un ICO.", "The file isn't an ICO.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        // A drawing scales to any size: never "too small".
        if super::svg::is_vector_file(f.input_path()) {
            return match super::svg::render_vector(f.input_path(), 64) {
                Ok((_, (w, h))) => {
                    let mut p = serde_json::Map::new();
                    p.insert("format".into(), json!(f.input_path().extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()));
                    p.insert("isVector".into(), json!(true));
                    p.insert("drawingWidth".into(), json!(w.round()));
                    p.insert("drawingHeight".into(), json!(h.round()));
                    Analysis::ok(p)
                }
                Err(e) => Analysis::status("failed", &e.code),
            };
        }
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
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "icon".into());
        let dir = ctx.file_dir(&f.id)?;
        let mut first = true;
        let mut emit = |file: &str, ext: &str, data: &[u8]| -> Result<(), FileError> {
            let out = dir.join(format!("{file}.{ext}"));
            write_file(&out, data)?;
            ctx.emit_output(&f.id, &out, ext, json!({ "file": file }), if first { in_bytes } else { 0 });
            first = false;
            Ok(())
        };
        ctx.progress(&f.id, 0.0, "decode", 1, 2);
        let kind = req.str_or("kind", "ico");
        if kind == "extract" {
            let frames = ico_frames(path)?;
            let n = frames.len();
            for (i, im) in frames.into_iter().enumerate() {
                ctx.check_cancel()?;
                ctx.progress(&f.id, 0.2 + 0.8 * i as f64 / n as f64, "encode", 2, 2);
                let (w, h) = im.dimensions();
                emit(&format!("{stem}-{w}x{h}"), "png", &img::encode_png(&im, false, 100)?)?;
            }
            ctx.progress(&f.id, 1.0, "encode", 2, 2);
            return Ok(Value::Null);
        }

        let src = if super::svg::is_vector_file(path) {
            // Drawn big once, then each size is made from it.
            super::svg::render_vector(path, if kind == "png" { 2048 } else { 1024 })?.0
        } else {
            img::load(path, req)?.img
        };
        let art = Art::from(req);
        ctx.progress(&f.id, 0.2, "encode", 2, 2);
        match kind {
            "favicon" => {
                let ico = encode_ico(&src, &[16, 32, 48], &art)?;
                emit("favicon", "ico", &ico)?;
                for s in [16u32, 32] {
                    emit(&format!("favicon-{s}x{s}"), "png", &png(&art.render(&src, s))?)?;
                }
                // Apple shows no transparency on the home screen: opaque, with a margin.
                let apple = Art { background: Some(art.background.unwrap_or([255, 255, 255])), padding: art.padding.max(0.08), ..art };
                emit("apple-touch-icon", "png", &png(&apple.render(&src, 180))?)?;
                for s in [192u32, 512] {
                    ctx.check_cancel()?;
                    emit(&format!("android-chrome-{s}x{s}"), "png", &png(&art.render(&src, s))?)?;
                }
                let name = match req.str_or("appName", "").trim() {
                    "" => stem.clone(),
                    n => n.to_string(),
                };
                let theme = req.str_or("themeColor", "#ffffff");
                let manifest = json!({
                    "name": name, "short_name": name,
                    "icons": [ { "src": "/android-chrome-192x192.png", "sizes": "192x192", "type": "image/png" },
                               { "src": "/android-chrome-512x512.png", "sizes": "512x512", "type": "image/png" } ],
                    "theme_color": theme, "background_color": theme, "display": "standalone"
                });
                emit("site", "webmanifest", serde_json::to_string_pretty(&manifest).unwrap_or_default().as_bytes())?;
                let html = "<link rel=\"icon\" href=\"/favicon.ico\" sizes=\"48x48\">\n\
                            <link rel=\"icon\" type=\"image/png\" sizes=\"32x32\" href=\"/favicon-32x32.png\">\n\
                            <link rel=\"icon\" type=\"image/png\" sizes=\"16x16\" href=\"/favicon-16x16.png\">\n\
                            <link rel=\"apple-touch-icon\" sizes=\"180x180\" href=\"/apple-touch-icon.png\">\n\
                            <link rel=\"manifest\" href=\"/site.webmanifest\">\n";
                emit("favicon", "html", html.as_bytes())?;
            }
            "png" => {
                let sizes = parse_sizes(req.str_or("sizes", ""), 4096)?;
                let n = sizes.len();
                for (i, s) in sizes.into_iter().enumerate() {
                    ctx.check_cancel()?;
                    ctx.progress(&f.id, 0.2 + 0.8 * i as f64 / n as f64, "encode", 2, 2);
                    emit(&format!("{stem}-{s}"), "png", &png(&art.render(&src, s))?)?;
                }
            }
            _ => {
                let sizes = parse_sizes(req.str_or("sizes", ""), 256)?;
                emit(&stem, "ico", &encode_ico(&src, &sizes, &art)?)?;
            }
        }
        ctx.progress(&f.id, 1.0, "encode", 2, 2);
        Ok(Value::Null)
    }
}

/// How the source is drawn into a square of a given size.
#[derive(Clone, Copy)]
struct Art {
    fit: Fit,
    padding: f64,
    background: Option<[u8; 3]>,
    filter: FilterType,
}

#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Pad,
    Crop,
    Stretch,
}

impl Art {
    fn from(req: &Request) -> Self {
        Art {
            fit: match req.str_or("fit", "pad") {
                "crop" => Fit::Crop,
                "stretch" => Fit::Stretch,
                _ => Fit::Pad,
            },
            padding: (req.f64_or("padding", 0.0).clamp(0.0, 40.0)) / 100.0,
            background: (req.str_or("background", "transparent") == "color").then(|| img::parse_color(req.str_or("backgroundColor", "#ffffff"))),
            filter: if req.str_or("filter", "lanczos3") == "nearest" { FilterType::Nearest } else { FilterType::Lanczos3 },
        }
    }

    fn render(&self, src: &DynamicImage, size: u32) -> RgbaImage {
        let bg = match self.background {
            Some(c) => Rgba([c[0], c[1], c[2], 255]),
            None => Rgba([0, 0, 0, 0]),
        };
        let mut canvas = RgbaImage::from_pixel(size, size, bg);
        let inner = ((size as f64) * (1.0 - 2.0 * self.padding)).round().max(1.0) as u32;
        let (w, h) = src.dimensions();
        let drawn = match self.fit {
            Fit::Stretch => img::resize(src, inner, inner, self.filter),
            Fit::Crop => {
                let side = w.min(h);
                img::resize(&src.crop_imm((w - side) / 2, (h - side) / 2, side, side), inner, inner, self.filter)
            }
            Fit::Pad => {
                // Keeps the shape, the long side `inner`.
                let s = inner as f64 / w.max(h) as f64;
                img::resize(src, ((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1), self.filter)
            }
        }
        .to_rgba8();
        let x = (size - drawn.width()) / 2;
        let y = (size - drawn.height()) / 2;
        image::imageops::overlay(&mut canvas, &drawn, x as i64, y as i64);
        canvas
    }
}

fn png(im: &RgbaImage) -> Result<Vec<u8>, FileError> {
    img::encode_png(&DynamicImage::ImageRgba8(im.clone()), false, 100)
}

fn encode_ico(src: &DynamicImage, sizes: &[u32], art: &Art) -> Result<Vec<u8>, FileError> {
    let images: Vec<RgbaImage> = sizes.iter().map(|s| art.render(src, *s)).collect();
    let frames = images
        .iter()
        .map(|im| IcoFrame::as_png(im.as_raw(), im.width(), im.height(), ExtendedColorType::Rgba8))
        .collect::<Result<Vec<_>, _>>()
        .map_err(img::img_err)?;
    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames).map_err(img::img_err)?;
    Ok(out)
}

/// "16, 32 48" → [16, 32, 48], sorted, without repeats.
fn parse_sizes(s: &str, max: u32) -> Result<Vec<u32>, FileError> {
    let mut v = Vec::new();
    for part in s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|p| !p.is_empty()) {
        // "32", "32px", "32 px", "32x32" all mean 32.
        let p = part.to_ascii_lowercase();
        let p = p.trim_end_matches("px");
        if p.is_empty() {
            continue;
        }
        let p = match p.split_once(['x', '×']) {
            Some((a, b)) if a == b => a,
            _ => p,
        };
        let n: u32 = p.parse().map_err(|_| FileError::with(codes::INVALID_OPTIONS, json!({ "part": part })))?;
        if n == 0 || n > max {
            return Err(FileError::with("ico_size_out_of_range", json!({ "size": n })));
        }
        v.push(n);
    }
    v.sort_unstable();
    v.dedup();
    if v.is_empty() {
        return Err(FileError::new("ico_no_sizes"));
    }
    Ok(v)
}

/// Every image stored in an .ico (PNG or BMP entries), as decoded pixels.
fn ico_frames(path: &Path) -> Result<Vec<DynamicImage>, FileError> {
    let data = std::fs::read(path).map_err(super::io_err)?;
    if data.len() < 6 || data[0..4] != [0, 0, 1, 0] {
        return Err(FileError::new("not_ico"));
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut out = Vec::new();
    for i in 0..count {
        let e = 6 + i * 16;
        if e + 16 > data.len() {
            break;
        }
        let size = u32::from_le_bytes([data[e + 8], data[e + 9], data[e + 10], data[e + 11]]) as usize;
        let offset = u32::from_le_bytes([data[e + 12], data[e + 13], data[e + 14], data[e + 15]]) as usize;
        let Some(entry) = data.get(offset..offset + size) else { continue };
        let im = if entry.starts_with(b"\x89PNG") {
            image::load_from_memory_with_format(entry, image::ImageFormat::Png).ok()
        } else {
            // A BMP entry: wrap it in a one-image ICO so the decoder reads it.
            let mut one = vec![0, 0, 1, 0, 1, 0];
            one.extend_from_slice(&data[e..e + 12]);
            one.extend_from_slice(&22u32.to_le_bytes());
            one.extend_from_slice(entry);
            image::load_from_memory_with_format(&one, image::ImageFormat::Ico).ok()
        };
        if let Some(im) = im {
            out.push(im);
        }
    }
    if out.is_empty() {
        return Err(FileError::with(codes::CORRUPT_FILE, json!({ "message": "no images in the ICO" })));
    }
    Ok(out)
}
