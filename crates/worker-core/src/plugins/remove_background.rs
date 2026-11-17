//! Remove background: an AI model finds the subject (IS-Net, Apache-2.0, fast; or
//! BiRefNet, MIT, finer hair and edges but several times slower); the background becomes
//! transparent, a colour, or a blur of itself. Runs on the graphics card with DirectML
//! when it can, else on the processor; nothing leaves the PC.

use super::image_convert::IMAGE_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{img, onnx};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{imageops, DynamicImage, GrayImage, Luma, Rgba, RgbaImage};
use ort::session::Session;
use ort::value::Tensor;
use serde_json::{json, Value};

const SIDE: u32 = 1024;

/// A model: its engine (folder) and file, how it wants the pixels, and whether its
/// output still needs a sigmoid.
struct Model {
    engine: &'static str,
    file: &'static str,
    mean: [f32; 3],
    std: [f32; 3],
    logits: bool,
}

const FAST: Model = Model { engine: "isnet", file: "isnet-general-use.onnx", mean: [0.5; 3], std: [1.0; 3], logits: false };
const DETAILED: Model = Model { engine: "birefnet-lite", file: "birefnet-lite.onnx", mean: [0.485, 0.456, 0.406], std: [0.229, 0.224, 0.225], logits: true };

fn model(req: &Request) -> &'static Model {
    if req.str_or("quality", "fast") == "detailed" { &DETAILED } else { &FAST }
}

pub struct RemoveBackground;

impl Plugin for RemoveBackground {
    fn id(&self) -> &'static str {
        "remove-background"
    }

    fn manifest(&self) -> Value {
        let opaque = json!({ "field": "background", "ne": "transparent" });
        json!({
            "name": t("Elimină fundalul", "Remove background"),
            "summary": t("Decupează automat subiectul", "Cuts out the subject by itself"),
            "keywords": { "ro": ["elimină fundal", "fundal transparent", "decupare", "șterge fundalul", "png transparent"],
                          "en": ["remove background", "transparent background", "cut out", "background remover", "transparent png"] },
            "category": "image",
            "icon": "wand",
            "requires": [ { "engine": FAST.engine, "when": { "field": "quality", "ne": "detailed" } },
                          { "engine": DETAILED.engine, "when": { "field": "quality", "eq": "detailed" } } ],
            "input": { "extensions": IMAGE_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["png", "webp", "jpg"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["width", "height", "hasAlpha"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "heavy", "diskFactor": 1.5 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "background", "type": "select", "label": t("Fundal nou", "New background"), "default": "transparent", "display": "chips",
                  "options": [ opt("transparent", "Transparent", "Transparent"), opt("color", "O culoare", "A colour"), opt("blur", "Fundalul, încețoșat", "The background, blurred") ] },
                { "key": "color", "type": "color", "label": t("Culoarea", "The colour"), "default": "#ffffff", "showIf": { "field": "background", "eq": "color" } },
                { "key": "blur", "type": "range", "label": t("Cât de încețoșat", "How blurred"), "min": 1, "max": 10, "step": 1, "default": 5,
                  "showIf": { "field": "background", "eq": "blur" } },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "display": "chips",
                  "default": [ { "when": opaque, "value": "jpg" }, { "value": "png" } ],
                  "options": [ opt("png", "PNG", "PNG"), opt("webp", "WebP", "WebP"),
                               { "value": "jpg", "label": t("JPG", "JPG"), "showIf": opaque } ] },
                { "key": "crop", "type": "toggle", "default": false, "label": t("Taie marginile goale din jurul subiectului", "Crop the empty space around the subject"),
                  "showIf": { "field": "background", "eq": "transparent" } },
                { "key": "quality", "type": "select", "advanced": true, "group": "more", "label": t("Model", "Model"), "default": "fast",
                  "options": [ opt("fast", "Rapid (recomandat)", "Fast (recommended)"), opt("detailed", "Cel mai fin la păr și margini (mai lent)", "Finest hair and edges (slower)") ],
                  "help": t("Modelul fin e o descărcare separată și durează câteva secunde pe fiecare poză.", "The fine model is a separate download and takes a few seconds per picture.") },
                { "key": "edges", "type": "select", "advanced": true, "group": "more", "label": t("Margini", "Edges"), "default": "soft",
                  "options": [ opt("soft", "Fine (păr, blană)", "Fine (hair, fur)"), opt("sharp", "Nete (obiecte, produse)", "Crisp (objects, products)") ] }
            ],
            "rules": [
                { "id": "slow", "severity": "info", "when": { "all": [ { "field": "quality", "eq": "detailed" }, { "files": "count", "gt": 10 } ] },
                  "message": t("Modelul fin durează câteva secunde pe fiecare poză.", "The fine model takes a few seconds per picture.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}-fără-fundal", "{name}-no-background") } },
            "errors": {
                "no_subject": t("Nu am găsit niciun subiect în imagine.", "No subject was found in the picture.")
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
        // One session for the whole batch: loading the model takes a moment.
        let mut session: Option<Session> = None;
        for f in &ctx.req.files {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || cut_out(ctx, f, &mut session));
        }
        Ok(())
    }
}

fn cut_out(ctx: &Ctx, f: &FileRef, session: &mut Option<Session>) -> Result<Value, FileError> {
    let req = ctx.req;
    let path = f.input_path();
    let in_bytes = file_size(path);
    ctx.progress(&f.id, 0.0, "read", 1, 3);
    let picture = img::load(path, req)?.img.to_rgba8();
    ctx.check_cancel()?;
    if session.is_none() {
        ctx.progress(&f.id, 0.1, "model", 1, 3);
        let m = model(req);
        let model = onnx::model(req, m.engine, m.file)?;
        let threads = req.limits.threads.max(1) as usize;
        let started = std::time::Instant::now();
        let (s, device) = onnx::session(req, &model, threads)?;
        eprintln!("remove-background: model ready on the {} in {:?}", if matches!(device, onnx::Device::Gpu) { "graphics card" } else { "processor" }, started.elapsed());
        *session = Some(s);
    }
    ctx.progress(&f.id, 0.2, "find", 2, 3);
    let started = std::time::Instant::now();
    let mask = find_subject(session.as_mut().unwrap(), model(req), &picture)?;
    eprintln!("remove-background: subject found in {:?}", started.elapsed());
    ctx.check_cancel()?;
    ctx.progress(&f.id, 0.85, "compose", 3, 3);
    let mask = if req.str_or("edges", "soft") == "sharp" { sharpen(&mask) } else { mask };
    if !mask.pixels().any(|p| p.0[0] > 128) {
        return Err(FileError::new("no_subject"));
    }
    let background = req.str_or("background", "transparent");
    let mut out = compose(&picture, &mask, background, req);
    if background == "transparent" && req.bool_or("crop", false) {
        out = crop_to_subject(&out);
    }
    let format = match req.str_or("format", "png") {
        "jpg" if background != "transparent" => "jpg",
        "webp" => "webp",
        _ => "png",
    };
    let im = DynamicImage::ImageRgba8(out);
    let bytes = match format {
        "jpg" => img::encode_jpeg(&DynamicImage::ImageRgb8(im.to_rgb8()), 92, None, None, [255, 255, 255])?,
        "webp" => img::encode_webp(&im, 90, false)?,
        _ => img::encode_png_quick(&im)?,
    };
    let dest = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
    write_file(&dest, &bytes)?;
    ctx.progress(&f.id, 1.0, "compose", 3, 3);
    ctx.emit_output(&f.id, &dest, format, json!({}), in_bytes);
    Ok(Value::Null)
}

/// The model's view of the subject: 0 background … 255 subject, at the picture's size.
fn find_subject(session: &mut Session, m: &Model, picture: &RgbaImage) -> Result<GrayImage, FileError> {
    let small = imageops::resize(picture, SIDE, SIDE, imageops::FilterType::Triangle);
    let plane = (SIDE * SIDE) as usize;
    let mut data = vec![0f32; 3 * plane];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            data[c * plane + i] = (p.0[c] as f32 / 255.0 - m.mean[c]) / m.std[c];
        }
    }
    let input = Tensor::from_array(([1usize, 3, SIDE as usize, SIDE as usize], data.into_boxed_slice())).map_err(FileError::engine)?;
    let name = session.inputs()[0].name().to_string();
    let outputs = session.run(ort::inputs![name => input]).map_err(FileError::engine)?;
    let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(FileError::engine)?;
    let (h, w) = (shape[shape.len() - 2] as u32, shape[shape.len() - 1] as u32);
    let values = &values[..(h * w) as usize];
    // Logits → 0…1, then stretched to the full range (as the models' authors do).
    let sig: Vec<f32> = if m.logits { values.iter().map(|v| 1.0 / (1.0 + (-v).exp())).collect() } else { values.to_vec() };
    let (lo, hi) = sig.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let span = (hi - lo).max(1e-6);
    let small_mask = GrayImage::from_fn(w, h, |x, y| Luma([(((sig[(y * w + x) as usize] - lo) / span) * 255.0).round() as u8]));
    Ok(imageops::resize(&small_mask, picture.width(), picture.height(), imageops::FilterType::Triangle))
}

/// Crisp edges for objects: a steep step around the middle instead of a soft fade.
fn sharpen(mask: &GrayImage) -> GrayImage {
    GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        let v = mask.get_pixel(x, y).0[0] as f32 / 255.0;
        let t = ((v - 0.4) / 0.2).clamp(0.0, 1.0);
        Luma([(t * t * (3.0 - 2.0 * t) * 255.0).round() as u8])
    })
}

fn compose(picture: &RgbaImage, mask: &GrayImage, background: &str, req: &Request) -> RgbaImage {
    let under: Option<RgbaImage> = match background {
        "color" => {
            let c = img::parse_color(req.str_or("color", "#ffffff"));
            Some(RgbaImage::from_pixel(picture.width(), picture.height(), Rgba([c[0], c[1], c[2], 255])))
        }
        "blur" => {
            let level = req.f64_or("blur", 5.0).clamp(1.0, 10.0) as f32;
            let sigma = picture.width().max(picture.height()) as f32 * 0.003 * level;
            // Blurred at a smaller size (much faster, looks the same).
            let k = (sigma / 4.0).max(1.0);
            let (w, h) = (((picture.width() as f32 / k) as u32).max(1), ((picture.height() as f32 / k) as u32).max(1));
            let small = imageops::resize(picture, w, h, imageops::FilterType::Triangle);
            let blurred = imageops::blur(&small, sigma / k);
            Some(imageops::resize(&blurred, picture.width(), picture.height(), imageops::FilterType::Triangle))
        }
        _ => None,
    };
    let mut out = picture.clone();
    for (x, y, p) in out.enumerate_pixels_mut() {
        let a = mask.get_pixel(x, y).0[0] as f32 / 255.0 * (p.0[3] as f32 / 255.0);
        match &under {
            None => p.0[3] = (a * 255.0).round() as u8,
            Some(u) => {
                let b = u.get_pixel(x, y).0;
                for (v, under) in p.0.iter_mut().zip(b).take(3) {
                    *v = (*v as f32 * a + under as f32 * (1.0 - a)).round() as u8;
                }
                p.0[3] = 255;
            }
        }
    }
    out
}

/// The subject with a small margin, without the transparent space around it.
fn crop_to_subject(im: &RgbaImage) -> RgbaImage {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in im.enumerate_pixels() {
        if p.0[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 {
        return im.clone();
    }
    let m = ((x1 - x0).max(y1 - y0) as f32 * 0.02) as u32;
    let (x0, y0) = (x0.saturating_sub(m), y0.saturating_sub(m));
    let (x1, y1) = ((x1 + m).min(im.width() - 1), (y1 + m).min(im.height() - 1));
    imageops::crop_imm(im, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
}
