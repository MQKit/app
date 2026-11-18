//! Erase things from photos (a passer-by, a wire, a date stamp): the parts painted over
//! in the app are filled in by LaMa (big-lama, Apache-2.0), as if they were never there.
//! Each painted area is filled from its surroundings at the model's 512 px, and only the
//! painted pixels change, with soft edges; the rest of the photo keeps every pixel.
//! On the processor: DirectML can't run LaMa's Fourier convolutions.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{b64, img, onnx};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{imageops, DynamicImage, GrayImage, Luma, RgbImage};
use ort::session::Session;
use ort::value::Tensor;
use serde_json::{json, Value};

pub struct PhotoErase;

const SIDE: u32 = 512;
const ENGINE: &str = "lama";
const FILE: &str = "lama_fp32.onnx";
const INPUTS: &[&str] = &["jpg", "jpeg", "jfif", "png", "webp", "bmp", "tif", "tiff", "heic", "heif", "avif"];

impl Plugin for PhotoErase {
    fn id(&self) -> &'static str {
        "photo-erase"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Ștergere obiecte din poze", "Erase objects from photos"),
            "summary": t("Pictezi peste, dispare", "Paint over it, it's gone"),
            "keywords": { "ro": ["șterge obiect", "elimină persoană", "retușare", "curăță poza", "inpainting"], "en": ["object remover", "remove person", "retouch", "clean up photo", "inpainting"] },
            "category": "image",
            "icon": "eraser",
            "requires": [ { "engine": ENGINE } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": false, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["jpg", "png", "webp"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["format", "width", "height"] },
            "ui": { "preview": "none", "estimate": false, "editor": "mask", "inputList": "thumbnails" },
            "resources": { "class": "heavy", "diskFactor": 1.2 },
            "fields": [
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "same", "display": "chips",
                  "options": [ opt("same", "Ca originalul", "As the original"), opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG") ] }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_curatat", "{name}_cleaned") } },
            "errors": {
                "no_mask": t("Pictează mai întâi peste ce vrei să ștergi.", "First paint over what you want to erase."),
                "bad_mask": t("Zona pictată nu a putut fi citită.", "The painted area couldn't be read.")
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

    /// The photo with the painted parts filled in (the mask comes in `options.mask`), at
    /// most `maxPx` on its longer side.
    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        let Some(f) = super::preview_file(req) else { return Ok(None) };
        let mask = req.options.get("mask").and_then(Value::as_str).unwrap_or("");
        let max = req.target.as_ref().and_then(|t| t.get("maxPx")).and_then(Value::as_u64).unwrap_or(900).clamp(64, 1600) as u32;
        let photo = img::load(f.input_path(), req)?.img;
        let photo = if photo.width().max(photo.height()) > max * 2 { photo.resize(max * 2, max * 2, imageops::FilterType::Triangle) } else { photo };
        let mut session = None;
        let out = erase(req, &mut session, &photo.to_rgb8(), mask, &|_| {})?;
        let out = DynamicImage::ImageRgb8(out);
        let out = if out.width().max(out.height()) > max { out.resize(max, max, imageops::FilterType::Triangle) } else { out };
        let png = img::encode_png_quick(&out)?;
        crate::common::preview::write_single(req, &png, "png").map(Some)
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        let mut session: Option<Session> = None;
        for f in &ctx.req.files {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || {
                let req = ctx.req;
                let in_bytes = file_size(f.input_path());
                ctx.progress(&f.id, 0.0, "read", 1, 3);
                let mask = f.item.get("mask").and_then(Value::as_str).unwrap_or("");
                if mask.is_empty() {
                    return Err(FileError::skipped("no_mask", json!({})));
                }
                let loaded = img::load(f.input_path(), req)?;
                let rgb = loaded.img.to_rgb8();
                ctx.check_cancel()?;
                let report = |x: f64| ctx.progress(&f.id, 0.1 + 0.8 * x, "fill", 2, 3);
                let out = erase(req, &mut session, &rgb, mask, &report)?;
                ctx.progress(&f.id, 0.92, "write", 3, 3);
                let src_ext = f.input_path().extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                let format = match req.str_or("format", "same") {
                    "png" => "png",
                    "jpg" => "jpg",
                    _ if matches!(src_ext.as_str(), "png" | "bmp" | "tif" | "tiff") => "png",
                    _ if src_ext == "webp" => "webp",
                    _ => "jpg",
                };
                let im = DynamicImage::ImageRgb8(out);
                let bytes = match format {
                    "png" => img::encode_png_quick(&im)?,
                    "webp" => img::encode_webp(&im, 92, false)?,
                    _ => img::encode_jpeg(&im, 94, loaded.exif.as_deref(), loaded.icc.as_deref(), [255, 255, 255])?,
                };
                let dest = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
                write_file(&dest, &bytes)?;
                ctx.progress(&f.id, 1.0, "write", 3, 3);
                ctx.emit_output(&f.id, &dest, format, json!({}), in_bytes);
                Ok(Value::Null)
            });
        }
        Ok(())
    }
}

/// The painted mask (a PNG data URL, any size; painted = not black or transparent), at
/// the photo's size, as 0/255.
fn read_mask(src: &str, w: u32, h: u32) -> Result<GrayImage, FileError> {
    let bytes = b64::data_url(src).ok_or_else(|| FileError::new("bad_mask"))?;
    let m = image::load_from_memory(&bytes).map_err(|_| FileError::new("bad_mask"))?.to_rgba8();
    let m = GrayImage::from_fn(m.width(), m.height(), |x, y| {
        let p = m.get_pixel(x, y).0;
        Luma([if p[3] > 40 && (p[0] as u16 + p[1] as u16 + p[2] as u16) > 60 { 255 } else { 0 }])
    });
    let m = imageops::resize(&m, w, h, imageops::FilterType::Triangle);
    Ok(GrayImage::from_fn(w, h, |x, y| Luma([if m.get_pixel(x, y).0[0] > 100 { 255 } else { 0 }])))
}

/// Grows the painted parts by `r` pixels (a square neighbourhood, done in two passes).
fn dilate(m: &GrayImage, r: u32) -> GrayImage {
    let (w, h) = m.dimensions();
    let mut a = GrayImage::new(w, h);
    for y in 0..h {
        let mut last: i64 = -(r as i64) - 1;
        // Forward then backward, remembering the nearest painted pixel in the row.
        for x in 0..w {
            if m.get_pixel(x, y).0[0] > 0 {
                last = x as i64;
            }
            if x as i64 - last <= r as i64 {
                a.put_pixel(x, y, Luma([255]));
            }
        }
        let mut next: i64 = i64::MAX;
        for x in (0..w).rev() {
            if m.get_pixel(x, y).0[0] > 0 {
                next = x as i64;
            }
            if next - (x as i64) <= r as i64 {
                a.put_pixel(x, y, Luma([255]));
            }
        }
    }
    let mut b = GrayImage::new(w, h);
    for x in 0..w {
        let mut last: i64 = -(r as i64) - 1;
        for y in 0..h {
            if a.get_pixel(x, y).0[0] > 0 {
                last = y as i64;
            }
            if y as i64 - last <= r as i64 {
                b.put_pixel(x, y, Luma([255]));
            }
        }
        let mut next: i64 = i64::MAX;
        for y in (0..h).rev() {
            if a.get_pixel(x, y).0[0] > 0 {
                next = y as i64;
            }
            if next - (y as i64) <= r as i64 {
                b.put_pixel(x, y, Luma([255]));
            }
        }
    }
    b
}

/// The bounding boxes of the separate painted areas (close ones merged), as x0, y0, x1, y1.
fn areas(m: &GrayImage) -> Vec<[u32; 4]> {
    // Found on a small copy (fast), then scaled back.
    let k = (m.width().max(m.height()) as f64 / 256.0).max(1.0);
    let (sw, sh) = ((m.width() as f64 / k).ceil() as u32, (m.height() as f64 / k).ceil() as u32);
    let small = imageops::resize(m, sw, sh, imageops::FilterType::Triangle);
    let mut seen = vec![false; (sw * sh) as usize];
    let mut boxes: Vec<[u32; 4]> = Vec::new();
    for sy in 0..sh {
        for sx in 0..sw {
            let i = (sy * sw + sx) as usize;
            if seen[i] || small.get_pixel(sx, sy).0[0] == 0 {
                continue;
            }
            let mut b = [sx, sy, sx, sy];
            let mut stack = vec![(sx, sy)];
            seen[i] = true;
            while let Some((x, y)) = stack.pop() {
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= sw as i64 || ny >= sh as i64 {
                        continue;
                    }
                    let j = (ny as u32 * sw + nx as u32) as usize;
                    if !seen[j] && small.get_pixel(nx as u32, ny as u32).0[0] > 0 {
                        seen[j] = true;
                        stack.push((nx as u32, ny as u32));
                    }
                }
            }
            boxes.push(b);
        }
    }
    // Areas whose boxes touch (with a little room) are done together.
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                if a[0] <= b[2] + 8 && b[0] <= a[2] + 8 && a[1] <= b[3] + 8 && b[1] <= a[3] + 8 {
                    boxes[i] = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                    boxes.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    boxes
        .into_iter()
        .map(|b| {
            let s = |v: u32| (v as f64 * k) as u32;
            [s(b[0]), s(b[1]), (s(b[2] + 1)).min(m.width()), (s(b[3] + 1)).min(m.height())]
        })
        .collect()
}

/// The photo with every painted area filled in.
fn erase(req: &Request, session: &mut Option<Session>, photo: &RgbImage, mask_src: &str, report: &dyn Fn(f64)) -> Result<RgbImage, FileError> {
    let (w, h) = photo.dimensions();
    let painted = read_mask(mask_src, w, h)?;
    if !painted.pixels().any(|p| p.0[0] > 0) {
        return Err(FileError::new("no_mask"));
    }
    // A little wider than painted: LaMa fills best when no edge of the object is left.
    let grow = ((w.max(h) as f64) * 0.004).round().max(3.0) as u32;
    let mask = dilate(&painted, grow);
    if session.is_none() {
        let threads = req.limits.threads.max(1) as usize;
        *session = Some(onnx::session_cpu(req, &onnx::model(req, ENGINE, FILE)?, threads)?);
    }
    let s = session.as_mut().unwrap();
    let mut out = photo.clone();
    let boxes = areas(&mask);
    for (i, b) in boxes.iter().enumerate() {
        report(i as f64 / boxes.len() as f64);
        // The area with as much around it again (at least 256 px), as a square.
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        let side = (bw.max(bh) * 2).max(256).min(w.max(h));
        let cx = (b[0] + b[2]) / 2;
        let cy = (b[1] + b[3]) / 2;
        let x0 = cx.saturating_sub(side / 2).min(w.saturating_sub(side.min(w)));
        let y0 = cy.saturating_sub(side / 2).min(h.saturating_sub(side.min(h)));
        let (cw, ch) = (side.min(w), side.min(h));
        let crop = imageops::crop_imm(&out, x0, y0, cw, ch).to_image();
        let crop_mask = imageops::crop_imm(&mask, x0, y0, cw, ch).to_image();
        let filled = fill(s, &crop, &crop_mask)?;
        // Soft edges: the filled part fades into the photo over a few pixels.
        let soft = imageops::blur(&crop_mask, (grow as f32 * 0.6).max(1.0));
        for y in 0..ch {
            for x in 0..cw {
                let a = soft.get_pixel(x, y).0[0] as f32 / 255.0;
                let a = if crop_mask.get_pixel(x, y).0[0] > 0 { a.max(0.5) } else { a };
                if a <= 0.0 {
                    continue;
                }
                let (p, q) = (out.get_pixel(x0 + x, y0 + y).0, filled.get_pixel(x, y).0);
                let mix = |c: usize| (p[c] as f32 * (1.0 - a) + q[c] as f32 * a).round().clamp(0.0, 255.0) as u8;
                out.put_pixel(x0 + x, y0 + y, image::Rgb([mix(0), mix(1), mix(2)]));
            }
        }
    }
    report(1.0);
    Ok(out)
}

/// One square through LaMa at 512 px, back at its own size.
fn fill(s: &mut Session, crop: &RgbImage, mask: &GrayImage) -> Result<RgbImage, FileError> {
    let (cw, ch) = crop.dimensions();
    let small = imageops::resize(crop, SIDE, SIDE, imageops::FilterType::CatmullRom);
    let small_mask = imageops::resize(mask, SIDE, SIDE, imageops::FilterType::Triangle);
    let plane = (SIDE * SIDE) as usize;
    let mut image = vec![0f32; 3 * plane];
    let mut m = vec![0f32; plane];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            image[c * plane + i] = p.0[c] as f32 / 255.0;
        }
    }
    for (i, p) in small_mask.pixels().enumerate() {
        m[i] = if p.0[0] > 60 { 1.0 } else { 0.0 };
    }
    // The inputs by their shape: the picture has 3 channels, the mask 1.
    let names: Vec<(String, bool)> = s.inputs().iter().map(|i| {
        let one = match i.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => shape.get(1) == Some(&1),
            _ => false,
        } || i.name().to_ascii_lowercase().contains("mask");
        (i.name().to_string(), one)
    }).collect();
    let (img_name, mask_name) = match names.as_slice() {
        [a, b] if a.1 && !b.1 => (b.0.clone(), a.0.clone()),
        [a, b] => (a.0.clone(), b.0.clone()),
        _ => return Err(FileError::engine("unexpected model inputs")),
    };
    let image = Tensor::from_array(([1usize, 3, SIDE as usize, SIDE as usize], image.into_boxed_slice())).map_err(FileError::engine)?;
    let m = Tensor::from_array(([1usize, 1, SIDE as usize, SIDE as usize], m.into_boxed_slice())).map_err(FileError::engine)?;
    let outputs = s.run(ort::inputs![img_name => image, mask_name => m]).map_err(FileError::engine)?;
    let (_, v) = outputs[0].try_extract_tensor::<f32>().map_err(FileError::engine)?;
    if v.len() < 3 * plane {
        return Err(FileError::engine("unexpected model output"));
    }
    // Some exports give 0–1, others 0–255.
    let scale = if v.iter().take(plane).fold(0f32, |a, b| a.max(*b)) > 2.0 { 1.0 } else { 255.0 };
    let out = RgbImage::from_fn(SIDE, SIDE, |x, y| {
        let i = (y * SIDE + x) as usize;
        image::Rgb([0, 1, 2].map(|c| (v[c * plane + i] * scale).round().clamp(0.0, 255.0) as u8))
    });
    Ok(imageops::resize(&out, cw, ch, imageops::FilterType::CatmullRom))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grow_and_find_areas() {
        let mut m = GrayImage::new(400, 300);
        for y in 50..60 {
            for x in 50..70 {
                m.put_pixel(x, y, Luma([255]));
            }
        }
        for y in 200..210 {
            for x in 300..320 {
                m.put_pixel(x, y, Luma([255]));
            }
        }
        let d = dilate(&m, 3);
        assert_eq!(d.get_pixel(47, 50).0[0], 255);
        assert_eq!(d.get_pixel(46, 50).0[0], 0);
        assert_eq!(d.get_pixel(50, 62).0[0], 255);
        let a = areas(&m);
        assert_eq!(a.len(), 2, "{a:?}");
        assert!(a.iter().any(|b| b[0] <= 50 && b[2] >= 70 && b[1] <= 50 && b[3] >= 60), "{a:?}");
    }
}
