//! AVIF and new formats: images to AVIF (much smaller at the same quality, transparency
//! kept), and AVIF or JPEG XL back to JPG, PNG or WebP.

use super::image_convert::IMAGE_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{ffmpeg, img};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::DynamicImage;
use serde_json::{json, Map, Value};
use std::path::Path;

pub struct Avif;

impl Plugin for Avif {
    fn id(&self) -> &'static str {
        "avif"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = IMAGE_INPUTS.iter().copied().chain(["jxl"]).collect();
        json!({
            "name": t("AVIF și formate noi", "AVIF and new formats"),
            "summary": t("Mai mici la aceeași calitate", "Smaller at the same quality"),
            "keywords": { "ro": ["avif", "jxl", "jpeg xl", "format nou", "imagini mici"], "en": ["avif", "jxl", "jpeg xl", "new format", "small images"] },
            "category": "image",
            "icon": "sparkles",
            "requires": [ { "engine": "ffmpeg", "when": { "any": [ { "field": "format", "eq": "avif" }, { "file": "format", "eq": "avif", "quant": "any" } ] } } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["avif", "jpg", "png", "webp"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "width", "height", "hasAlpha"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "heavy", "diskFactor": 1.0 },
            "presets": [
                { "id": "toAvif", "label": t("În AVIF (mai mic)", "To AVIF (smaller)"), "default": true, "values": { "format": "avif", "quality": 60 } },
                { "id": "toJpg", "label": t("AVIF / JXL în JPG", "AVIF / JXL to JPG"), "values": { "format": "jpg", "quality": 90 } }
            ],
            "groups": [],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "avif", "display": "chips",
                  "options": [ opt("avif", "AVIF", "AVIF"), opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG"), opt("webp", "WebP", "WebP") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 60,
                  "help": t("La AVIF, 60 arată ca un JPG de 90, la o fracțiune din mărime.", "For AVIF, 60 looks like a JPG at 90, at a fraction of the size."),
                  "showIf": { "field": "format", "ne": "png" } },
                { "key": "speed", "type": "select", "label": t("Viteză", "Speed"), "default": 6, "showIf": { "field": "format", "eq": "avif" },
                  "options": [ opt(8, "Rapid", "Fast"), opt(6, "Echilibrat", "Balanced"), opt(3, "Lent, puțin mai mic", "Slow, a little smaller") ] }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {}
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let path = f.input_path();
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext == "jxl" || ext == "avif" {
            let mut p = Map::new();
            p.insert("format".into(), json!(ext));
            if let Ok(im) = decode_new(path, req) {
                p.insert("width".into(), json!(im.width()));
                p.insert("height".into(), json!(im.height()));
                p.insert("hasAlpha".into(), json!(im.color().has_alpha()));
            }
            return Analysis::ok(p);
        }
        match img::analyze(path) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "decode", 1, 2);
        std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
        let im = decode_new(path, req)?;
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.3, "encode", 2, 2);
        let format = req.str_or("format", "avif");
        let q = req.f64_or("quality", 60.0).clamp(1.0, 100.0);
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        match format {
            "avif" => {
                // FFmpeg with libaom: still picture, quality as CRF (0 best, 63 smallest).
                let png = ctx.work_dir.join(format!("in-{}.png", f.id));
                // A temporary file for FFmpeg: written fast, not small.
                write_file(&png, &img::encode_png_quick(&im)?)?;
                let crf = (63.0 - q * 0.6).round().clamp(0.0, 63.0) as i64;
                let speed = req.f64_or("speed", 6.0).clamp(0.0, 9.0) as i64;
                let mut a: Vec<String> = vec!["-i".into(), png.to_string_lossy().into_owned()];
                if im.color().has_alpha() {
                    // Colour and transparency as two pictures in one AVIF.
                    a.extend(["-filter_complex".into(), "[0:v]format=yuva444p,split[c][a];[a]alphaextract[al]".into(), "-map".into(), "[c]".into(), "-map".into(), "[al]".into()]);
                } else {
                    a.extend(["-vf".into(), "format=yuv420p".into()]);
                }
                a.extend(["-c:v".into(), "libaom-av1".into(), "-still-picture".into(), "1".into(), "-crf".into(), crf.to_string(), "-b:v".into(), "0".into(),
                          "-cpu-used".into(), speed.to_string(), "-row-mt".into(), "1".into(), out.to_string_lossy().into_owned()]);
                ffmpeg::run(ctx, &f.id, &a, 0.0, 2, 2)?;
            }
            "png" => {
                let big = im.width() as u64 * im.height() as u64 > 6_000_000;
                write_file(&out, &if big { img::encode_png_quick(&im)? } else { img::encode_png(&im, false, 100)? })?
            }
            "webp" => write_file(&out, &img::encode_webp(&im, q as u8, false)?)?,
            _ => write_file(&out, &img::encode_jpeg(&im, q as u8, None, None, [255, 255, 255])?)?,
        }
        ctx.progress(&f.id, 1.0, "encode", 2, 2);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

/// Any input as pixels: JPEG XL with jxl-oxide, AVIF with FFmpeg (no Windows extension
/// needed), the rest with the shared image loader.
fn decode_new(path: &Path, req: &Request) -> Result<DynamicImage, FileError> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jxl" => {
            let file = std::io::BufReader::new(std::fs::File::open(path).map_err(super::io_err)?);
            let dec = jxl_oxide::integration::JxlDecoder::new(file).map_err(img::img_err)?;
            let (w, h) = image::ImageDecoder::dimensions(&dec);
            img::check_size(w, h, req.limits.max_megapixels)?;
            DynamicImage::from_decoder(dec).map_err(img::img_err)
        }
        "avif" => {
            let work = req.work_dir.clone().unwrap_or_else(std::env::temp_dir);
            std::fs::create_dir_all(&work).map_err(super::io_err)?;
            let tmp = work.join(format!("avif-{}.png", std::process::id()));
            let exe = ffmpeg::exe(req, "ffmpeg")?;
            let run = |merge_alpha: bool| {
                let mut c = std::process::Command::new(&exe);
                c.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(path);
                if merge_alpha {
                    // An AVIF with transparency holds it as a second picture.
                    c.args(["-filter_complex", "[0:v:0][0:v:1]alphamerge"]);
                }
                c.args(["-frames:v", "1", "-pix_fmt", "rgba"]).arg(&tmp).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                let limit = crate::common::proc::limit_for(super::file_size(path), std::time::Duration::from_secs(120), std::time::Duration::from_secs(5), std::time::Duration::from_secs(900));
                match c.spawn() {
                    Ok(mut child) => crate::common::proc::wait(&mut child, None, "ffmpeg", limit, || {}).map(|s| s.success()),
                    Err(_) => Ok(false),
                }
            };
            if !run(true)? && !run(false)? {
                return Err(FileError::new(codes::CORRUPT_FILE));
            }
            let im = image::open(&tmp).map_err(img::img_err)?;
            let _ = std::fs::remove_file(&tmp);
            Ok(im)
        }
        _ => Ok(img::load(path, req)?.img),
    }
}
