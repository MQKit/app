//! Pictures into one animation, in the order given: a GIF (with a palette made for
//! these pictures), an animated WebP or a short MP4. Each picture is fitted to one
//! frame size, whole on a background or cropped to fill it.

use super::{Analysis, Ctx, Plugin};
use crate::common::{ffmpeg, img};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{imageops::FilterType, DynamicImage, GenericImageView, Rgba, RgbaImage};
use serde_json::{json, Value};

pub struct ImagesToGif;

impl Plugin for ImagesToGif {
    fn id(&self) -> &'static str {
        "images-to-gif"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Animație din poze", "Animation from pictures"),
            "summary": t("GIF, WebP sau MP4", "GIF, WebP or MP4"),
            "keywords": { "ro": ["gif din poze", "animație", "slideshow", "poze în gif", "gif"], "en": ["gif maker", "animation", "slideshow", "images to gif", "gif"] },
            "category": "image",
            "icon": "slideshow",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": super::image_convert::IMAGE_INPUTS, "multiple": true, "folders": true, "mode": "combined", "ordered": true, "minFiles": 2 },
            "output": { "kind": "single", "extensions": ["gif", "webp", "mp4"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "width", "height"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 2.0 },
            "presets": [
                { "id": "gif", "label": t("GIF", "GIF"), "default": true, "values": { "format": "gif", "frameTime": 0.5 } },
                { "id": "slideshow", "label": t("Prezentare MP4", "MP4 slideshow"), "values": { "format": "mp4", "frameTime": 2, "width": 1080 } },
                { "id": "fast", "label": t("Animație rapidă", "Fast animation"), "values": { "format": "gif", "frameTime": 0.1 } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Rezultat", "Result"), "default": "gif", "display": "chips",
                  "options": [ opt("gif", "GIF", "GIF"), opt("webp", "WebP animat", "Animated WebP"), opt("mp4", "MP4", "MP4") ] },
                { "key": "frameTime", "type": "select", "label": t("Cât stă fiecare poză", "How long each picture shows"), "default": 0.5,
                  "options": [ opt(0.1, "0,1 s", "0.1 s"), opt(0.2, "0,2 s", "0.2 s"), opt(0.5, "0,5 s", "0.5 s"), opt(1, "1 s", "1 s"),
                               opt(2, "2 s", "2 s"), opt(3, "3 s", "3 s"), opt(5, "5 s", "5 s") ] },
                { "key": "width", "type": "select", "label": t("Lățime", "Width"), "default": 480,
                  "options": [ opt(320, "320 px", "320 px"), opt(480, "480 px", "480 px"), opt(640, "640 px", "640 px"), opt(800, "800 px", "800 px"),
                               opt(1080, "1080 px", "1080 px"), opt(0, "Ca prima poză", "As the first picture") ] },
                { "key": "fit", "type": "select", "label": t("Poze de alte forme", "Pictures of other shapes"), "default": "pad", "display": "chips",
                  "options": [ opt("pad", "Întregi", "Whole"), opt("crop", "Umplu cadrul", "Fill the frame") ] },
                { "key": "background", "type": "color", "label": t("Fundal", "Background"), "default": "#ffffff", "showIf": { "field": "fit", "eq": "pad" } },
                { "key": "loop", "type": "toggle", "default": true, "label": t("Se repetă la nesfârșit", "Loops forever"), "showIf": { "field": "format", "ne": "mp4" } },
                { "key": "pingPong", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Înainte și înapoi", "Forward and back") },
                { "key": "colors", "type": "select", "advanced": true, "group": "more", "label": t("Culori", "Colors"), "default": 256,
                  "showIf": { "field": "format", "eq": "gif" },
                  "options": [ opt(256, "256 (toate)", "256 (all)"), opt(128, "128", "128"), opt(64, "64 (fișier mic)", "64 (small file)"), opt(32, "32 (foarte mic)", "32 (very small)") ] }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_animatie", "{name}_animation") } },
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

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        let files = &ctx.req.files;
        ctx.run_one("*", || build(ctx, files));
        Ok(())
    }
}

/// `im` made exactly `w`×`h`: whole on `bg`, or scaled to fill and cropped in the middle.
fn fit(im: &DynamicImage, w: u32, h: u32, crop: bool, bg: [u8; 3]) -> RgbaImage {
    let (iw, ih) = im.dimensions();
    let k = if crop { (w as f64 / iw as f64).max(h as f64 / ih as f64) } else { (w as f64 / iw as f64).min(h as f64 / ih as f64) };
    let (sw, sh) = (((iw as f64 * k).round() as u32).max(1), ((ih as f64 * k).round() as u32).max(1));
    let scaled = im.resize_exact(sw, sh, FilterType::Lanczos3).to_rgba8();
    let mut out = RgbaImage::from_pixel(w, h, Rgba([bg[0], bg[1], bg[2], 255]));
    let (x, y) = (w as i64 / 2 - sw as i64 / 2, h as i64 / 2 - sh as i64 / 2);
    // Transparent pictures land on the background, not on black.
    image::imageops::overlay(&mut out, &scaled, x, y);
    out
}

fn build(ctx: &Ctx, files: &[FileRef]) -> Result<Value, FileError> {
    let req = ctx.req;
    let format = req.str_or("format", "gif");
    let crop = req.str_or("fit", "pad") == "crop";
    let bg = img::parse_color(req.str_or("background", "#ffffff"));
    let frame_time = req.f64_or("frameTime", 0.5).clamp(0.02, 30.0);
    let dir = ctx.work_dir.join("frames");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(super::io_err)?;

    let mut size: Option<(u32, u32)> = None;
    let mut in_bytes = 0u64;
    let mut count = 0usize;
    for (i, f) in files.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.progress("*", 0.6 * i as f64 / files.len() as f64, "decode", 1, 2);
        in_bytes += super::file_size(f.input_path());
        let im = img::load(f.input_path(), req)?.img;
        // The frame: the first picture's shape, at the width chosen (never wider than it,
        // and even, as video needs).
        let (w, h) = *size.get_or_insert_with(|| {
            let (iw, ih) = im.dimensions();
            let want = req.f64_or("width", 480.0) as u32;
            let w = if want == 0 { iw.min(1920) } else { want.min(iw) };
            let h = (w as f64 * ih as f64 / iw as f64).round() as u32;
            ((w / 2 * 2).max(2), (h / 2 * 2).max(2))
        });
        fit(&im, w, h, crop, bg).save(dir.join(format!("f{count:05}.png"))).map_err(img::img_err)?;
        count += 1;
    }
    // Forward and back: the pictures again in reverse, without repeating the two ends.
    if req.bool_or("pingPong", false) && count > 2 {
        for j in (1..count - 1).rev() {
            std::fs::copy(dir.join(format!("f{j:05}.png")), dir.join(format!("f{count:05}.png"))).map_err(super::io_err)?;
            count += 1;
        }
    }
    let rate = format!("{:.6}", 1.0 / frame_time);
    let pattern = dir.join("f%05d.png").to_string_lossy().into_owned();
    let mut a: Vec<String> = ["-framerate", &rate, "-i", &pattern].iter().map(|s| s.to_string()).collect();
    let loops = if req.bool_or("loop", true) { "0" } else { "-1" };
    match format {
        "mp4" => {
            // Players expect a usual frame rate: each picture is held for its time.
            a.extend(["-vf", "fps=30,format=yuv420p", "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-movflags", "+faststart"].iter().map(|s| s.to_string()));
        }
        "webp" => {
            a.extend(["-c:v", "libwebp_anim", "-quality", "85", "-compression_level", "5", "-loop", loops].iter().map(|s| s.to_string()));
        }
        _ => {
            let colors = req.f64_or("colors", 256.0).clamp(4.0, 256.0) as i64;
            let vf = format!("[0:v]split[s0][s1];[s0]palettegen=stats_mode=full:max_colors={colors}[p];[s1][p]paletteuse=dither=sierra2_4a[g]");
            a.extend(["-filter_complex".to_string(), vf, "-map".into(), "[g]".into(), "-loop".into(), loops.into()]);
        }
    }
    let out = ctx.file_dir("combined")?.join(format!("result.{format}"));
    a.push(out.to_string_lossy().into_owned());
    ffmpeg::run(ctx, "*", &a, count as f64 * frame_time, 2, 2)?;
    let _ = std::fs::remove_dir_all(&dir);
    ctx.progress("*", 1.0, "encode", 2, 2);
    ctx.emit_output("*", &out, format, json!({ "frames": count }), in_bytes);
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting() {
        let wide = DynamicImage::ImageRgba8(RgbaImage::from_pixel(200, 100, Rgba([255, 0, 0, 255])));
        // Whole: red band in the middle, white above and below.
        let o = fit(&wide, 100, 100, false, [255, 255, 255]);
        assert_eq!(o.get_pixel(50, 5).0, [255, 255, 255, 255]);
        assert_eq!(o.get_pixel(50, 50).0, [255, 0, 0, 255]);
        // Filling: red everywhere.
        let o = fit(&wide, 100, 100, true, [255, 255, 255]);
        assert_eq!(o.get_pixel(50, 5).0, [255, 0, 0, 255]);
        assert_eq!(o.dimensions(), (100, 100));
    }
}
