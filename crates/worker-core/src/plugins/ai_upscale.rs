//! AI upscale: Real-ESRGAN (BSD-3-Clause, Qualcomm's ONNX exports) makes pictures 2, 3
//! or 4 times bigger with real detail instead of blur. The model works on 128-pixel
//! tiles; each tile overlaps its neighbours and only its middle is kept, so there are
//! no seams. Transparency is enlarged the classic way. Graphics card (DirectML) when
//! there is one, else the processor.

use super::image_convert::IMAGE_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{img, onnx};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{imageops, DynamicImage, GrayImage, RgbImage, RgbaImage};
use ort::session::Session;
use ort::value::Tensor;
use serde_json::{json, Value};

/// The model's tile, its overlap on each side, and how much bigger it makes it.
const TILE: u32 = 128;
const PAD: u32 = 12;
const SCALE: u32 = 4;

struct Model {
    engine: &'static str,
    file: &'static str,
}

const FAST: Model = Model { engine: "esrgan-general", file: "real_esrgan_general_x4v3-onnx-float/real_esrgan_general_x4v3.onnx" };
const DETAILED: Model = Model { engine: "esrgan-x4plus", file: "real_esrgan_x4plus-onnx-float/real_esrgan_x4plus.onnx" };

fn model(req: &Request) -> &'static Model {
    if req.str_or("model", "fast") == "detailed" { &DETAILED } else { &FAST }
}

pub struct AiUpscale;

impl Plugin for AiUpscale {
    fn id(&self) -> &'static str {
        "ai-upscale"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Mărire cu AI", "AI upscale"),
            "summary": t("Poze mai mari, cu detalii", "Bigger pictures, with detail"),
            "keywords": { "ro": ["mărire poză", "upscale", "rezoluție mai mare", "poză clară", "îmbunătățire poză"],
                          "en": ["upscale", "enlarge picture", "higher resolution", "sharpen photo", "enhance photo"] },
            "category": "image",
            "icon": "zoom-in",
            "requires": [ { "engine": FAST.engine, "when": { "field": "model", "ne": "detailed" } },
                          { "engine": DETAILED.engine, "when": { "field": "model", "eq": "detailed" } } ],
            "input": { "extensions": IMAGE_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["png", "jpg", "webp"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["width", "height", "hasAlpha", "format"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "heavy", "diskFactor": 8.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "scale", "type": "select", "label": t("De câte ori mai mare", "How many times bigger"), "default": 2, "display": "chips",
                  "options": [ opt(2, "2×", "2×"), opt(3, "3×", "3×"), opt(4, "4×", "4×") ] },
                { "key": "model", "type": "select", "label": t("Model", "Model"), "default": "fast",
                  "options": [ opt("fast", "Rapid (recomandat)", "Fast (recommended)"), opt("detailed", "Mai multe detalii (mult mai lent fără placă video)", "More detail (much slower without a graphics card)") ] },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "same", "advanced": true, "group": "more",
                  "options": [ opt("same", "Ca originalul", "Same as the original"), opt("png", "PNG", "PNG"), opt("jpg", "JPG", "JPG"), opt("webp", "WebP", "WebP") ] }
            ],
            "rules": [
                { "id": "big", "severity": "warning", "when": { "file": "width", "gt": 3000, "quant": "any" },
                  "message": t("Pozele mari ies uriașe și durează mult. Mărirea cu AI e făcută pentru poze mici sau vechi.",
                               "Big pictures come out huge and take long. AI upscale is made for small or old pictures.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}-mărit", "{name}-upscaled") } },
            "errors": {
                "too_big": t("Rezultatul ar avea {megapixels} megapixeli, peste limita de {limit}. Alege o mărire mai mică.",
                             "The result would be {megapixels} megapixels, over the {limit} limit. Choose a smaller scale.")
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

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        let mut session: Option<Session> = None;
        for f in &ctx.req.files {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || upscale(ctx, f, &mut session));
        }
        Ok(())
    }
}

fn upscale(ctx: &Ctx, f: &FileRef, session: &mut Option<Session>) -> Result<Value, FileError> {
    let req = ctx.req;
    let path = f.input_path();
    let in_bytes = file_size(path);
    ctx.progress(&f.id, 0.0, "read", 1, 3);
    let loaded = img::load(path, req)?;
    let picture = loaded.img;
    let scale = req.f64_or("scale", 2.0).clamp(2.0, 4.0).round() as u32;
    let (w, h) = (picture.width() * scale, picture.height() * scale);
    let limit = req.limits.max_megapixels.max(1) as u64;
    let megapixels = (w as u64 * h as u64).div_ceil(1_000_000);
    if megapixels > limit {
        return Err(FileError::with("too_big", json!({ "megapixels": megapixels, "limit": limit })));
    }
    if session.is_none() {
        ctx.progress(&f.id, 0.02, "model", 1, 3);
        let m = model(req);
        let file = onnx::model(req, m.engine, m.file)?;
        let (s, device) = onnx::session(req, &file, req.limits.threads.max(1) as usize)?;
        eprintln!("ai-upscale: model ready on the {}", if matches!(device, onnx::Device::Gpu) { "graphics card" } else { "processor" });
        *session = Some(s);
    }
    let started = std::time::Instant::now();
    let rgb = picture.to_rgb8();
    let big = enlarge(ctx, &f.id, session.as_mut().unwrap(), &rgb)?;
    eprintln!("ai-upscale: {}×{} enlarged in {:?}", rgb.width(), rgb.height(), started.elapsed());
    ctx.progress(&f.id, 0.92, "save", 3, 3);
    // From 4× down to the chosen size, smoothly.
    let big = if scale == SCALE { big } else { imageops::resize(&big, w, h, imageops::FilterType::Lanczos3) };
    let has_alpha = picture.color().has_alpha();
    let out = if has_alpha {
        let alpha: GrayImage = imageops::resize(&DynamicImage::ImageLuma8(alpha_of(&picture.to_rgba8())).to_luma8(), w, h, imageops::FilterType::CatmullRom);
        let mut rgba = RgbaImage::new(w, h);
        for ((o, c), a) in rgba.pixels_mut().zip(big.pixels()).zip(alpha.pixels()) {
            o.0 = [c.0[0], c.0[1], c.0[2], a.0[0]];
        }
        DynamicImage::ImageRgba8(rgba)
    } else {
        DynamicImage::ImageRgb8(big)
    };
    let source = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let format = match req.str_or("format", "same") {
        "png" => "png",
        "jpg" => "jpg",
        "webp" => "webp",
        _ => match source.as_str() {
            "jpg" | "jpeg" | "jfif" | "heic" | "heif" | "avif" if !has_alpha => "jpg",
            "webp" => "webp",
            _ => "png",
        },
    };
    let bytes = match format {
        "jpg" => img::encode_jpeg(&out, 92, loaded.exif.as_deref(), loaded.icc.as_deref(), [255, 255, 255])?,
        "webp" => img::encode_webp(&out, 92, false)?,
        _ => img::encode_png_quick(&out)?,
    };
    let dest = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
    write_file(&dest, &bytes)?;
    ctx.progress(&f.id, 1.0, "save", 3, 3);
    ctx.emit_output(&f.id, &dest, format, json!({}), in_bytes);
    Ok(Value::Null)
}

fn alpha_of(im: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(im.width(), im.height(), |x, y| image::Luma([im.get_pixel(x, y).0[3]]))
}

/// 4× bigger, tile by tile: each 128-pixel tile reaches 12 pixels into its neighbours
/// (edge pixels repeated at the borders), and only its middle 104 pixels are kept.
fn enlarge(ctx: &Ctx, file: &str, session: &mut Session, im: &RgbImage) -> Result<RgbImage, FileError> {
    let (w, h) = im.dimensions();
    let core = TILE - 2 * PAD;
    let cols = w.div_ceil(core);
    let rows = h.div_ceil(core);
    let total = (cols * rows) as f64;
    let mut out = RgbImage::new(w * SCALE, h * SCALE);
    let plane = (TILE * TILE) as usize;
    let name = session.inputs()[0].name().to_string();
    let mut done = 0.0;
    for ty in 0..rows {
        for tx in 0..cols {
            ctx.check_cancel()?;
            let (x0, y0) = (tx * core, ty * core);
            let mut data = vec![0f32; 3 * plane];
            for j in 0..TILE {
                let sy = (y0 as i64 + j as i64 - PAD as i64).clamp(0, h as i64 - 1) as u32;
                for i in 0..TILE {
                    let sx = (x0 as i64 + i as i64 - PAD as i64).clamp(0, w as i64 - 1) as u32;
                    let p = im.get_pixel(sx, sy).0;
                    let k = (j * TILE + i) as usize;
                    data[k] = p[0] as f32 / 255.0;
                    data[plane + k] = p[1] as f32 / 255.0;
                    data[2 * plane + k] = p[2] as f32 / 255.0;
                }
            }
            let input = Tensor::from_array(([1usize, 3, TILE as usize, TILE as usize], data.into_boxed_slice())).map_err(FileError::engine)?;
            let outputs = session.run(ort::inputs![name.as_str() => input]).map_err(FileError::engine)?;
            let (_, v) = outputs[0].try_extract_tensor::<f32>().map_err(FileError::engine)?;
            let side = (TILE * SCALE) as usize;
            let big_plane = side * side;
            // Keep the middle of the tile, as far as the picture goes.
            let keep_w = core.min(w - x0) * SCALE;
            let keep_h = core.min(h - y0) * SCALE;
            for j in 0..keep_h {
                for i in 0..keep_w {
                    let k = ((PAD * SCALE + j) as usize) * side + (PAD * SCALE + i) as usize;
                    let px = |c: usize| (v[c * big_plane + k].clamp(0.0, 1.0) * 255.0).round() as u8;
                    out.put_pixel(x0 * SCALE + i, y0 * SCALE + j, image::Rgb([px(0), px(1), px(2)]));
                }
            }
            done += 1.0;
            ctx.progress(file, 0.05 + 0.85 * done / total, "enlarge", 2, 3);
        }
    }
    Ok(out)
}
