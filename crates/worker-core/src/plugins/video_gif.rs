//! Video to GIF (or animated WebP), with a palette made for the clip; and GIF to MP4,
//! which is often ten times smaller.

use super::video_compress::{time_opt, VIDEO_INPUTS};
use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct VideoGif;

impl Plugin for VideoGif {
    fn id(&self) -> &'static str {
        "video-gif"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = VIDEO_INPUTS.iter().copied().chain(["gif"]).collect();
        let to_anim = json!({ "field": "format", "in": ["gif", "webp"] });
        json!({
            "name": t("Video în GIF", "Video to GIF"),
            "summary": t("Un clip care se repetă", "A clip that loops"),
            "keywords": { "ro": ["gif", "animație", "meme", "webp animat", "gif în mp4"], "en": ["gif", "animation", "meme", "animated webp", "gif to mp4"] },
            "category": "video",
            "icon": "gif",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["gif", "webp", "mp4"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasVideo", "width", "height"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 1.0 },
            "presets": [
                { "id": "chat", "label": t("Pentru chat", "For chat"), "default": true, "values": { "format": "gif", "width": 480, "fps": 15 } },
                { "id": "small", "label": t("Mic", "Small"), "values": { "format": "gif", "width": 320, "fps": 10 } },
                { "id": "sharp", "label": t("Clar, WebP", "Sharp, WebP"), "values": { "format": "webp", "width": 720, "fps": 24 } },
                { "id": "tomp4", "label": t("GIF în MP4", "GIF to MP4"), "values": { "format": "mp4" } }
            ],
            "groups": [ { "id": "cut", "label": t("Doar o parte", "Only a part"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Rezultat", "Result"), "default": "gif", "display": "chips",
                  "options": [ opt("gif", "GIF", "GIF"), opt("webp", "WebP animat", "Animated WebP"), opt("mp4", "MP4 (din GIF)", "MP4 (from a GIF)") ] },
                { "key": "width", "type": "select", "label": t("Lățime", "Width"), "default": 480, "showIf": to_anim,
                  "options": [ opt(240, "240 px", "240 px"), opt(320, "320 px", "320 px"), opt(480, "480 px", "480 px"), opt(640, "640 px", "640 px"),
                               opt(720, "720 px", "720 px"), opt(0, "Ca originalul", "As original") ] },
                { "key": "fps", "type": "select", "label": t("Cadre pe secundă", "Frames per second"), "default": 15, "showIf": to_anim,
                  "options": [ opt(10, "10 (fișier mic)", "10 (small file)"), opt(15, "15", "15"), opt(20, "20", "20"), opt(24, "24 (fluid)", "24 (smooth)") ] },
                { "key": "loop", "type": "toggle", "default": true, "label": t("Se repetă la nesfârșit", "Loops forever"), "showIf": to_anim },
                { "key": "start", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("De la", "From"), "placeholder": t("început", "start") },
                { "key": "end", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("Până la", "To"), "placeholder": t("sfârșit", "end") }
            ],
            "rules": [
                { "id": "long_clip", "severity": "warning", "when": { "all": [ to_anim, { "file": "durationSec", "gt": 20 } ] },
                  "message": t("Un GIF lung iese foarte mare. Alege doar câteva secunde (Doar o parte).", "A long GIF gets very large. Pick just a few seconds (Only a part).") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": { "no_video": t("Fișierul nu are imagine.", "The file has no picture.") }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        match ffmpeg::probe(req, f.input_path()) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) if e.code == codes::CORRUPT_FILE => Analysis::status("corrupt", codes::CORRUPT_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "encode", 1, 1);
        let info = if f.analysis.contains_key("durationSec") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        if info.get("hasVideo").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_video"));
        }
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let start = time_opt(req, "start").unwrap_or(0.0).max(0.0);
        let end = time_opt(req, "end").filter(|e| *e > 0.0).map(|e| if duration > 0.0 { e.min(duration) } else { e }).unwrap_or(duration);
        let format = req.str_or("format", "gif");
        let mut a = Vec::new();
        if start > 0.0 {
            a.extend(sv(&["-ss", &format!("{start:.3}")]));
        }
        if end > start && end < duration {
            a.extend(sv(&["-t", &format!("{:.3}", end - start)]));
        }
        a.extend(["-i".into(), input.to_string_lossy().into_owned(), "-an".into()]);
        if format != "gif" {
            a.extend(sv(&["-map", "0:v:0"]));
        }
        let width = req.f64_or("width", 480.0) as i64;
        let fps = req.f64_or("fps", 15.0);
        // Never wider than the source.
        let src_w = info.get("video").and_then(|v| v["width"].as_i64()).unwrap_or(0);
        let mut scale = if width > 0 && (src_w == 0 || width < src_w) { format!(",scale={width}:-2:flags=lanczos") } else { String::new() };
        // HDR phone videos: tone mapped, or the GIF comes out grey.
        if ffmpeg::is_hdr(&info) {
            scale.push(',');
            scale.push_str(&ffmpeg::to_sdr(if format == "gif" { "rgb24" } else { "yuv420p" }));
        }
        let loops = if req.bool_or("loop", true) { "0" } else { "-1" };
        match format {
            "mp4" => a.extend(sv(&["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p", "-c:v", "libx264", "-crf", "20", "-preset", "medium", "-movflags", "+faststart"])),
            "webp" => a.extend(sv(&["-vf", &format!("fps={fps}{scale}"), "-c:v", "libwebp_anim", "-quality", "80", "-compression_level", "5", "-loop", loops])),
            _ => {
                // A palette made from this clip, then dithered: far better colors than the default.
                let vf = format!("[0:v:0]fps={fps}{scale},split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle[g]");
                a.extend(["-filter_complex".into(), vf, "-map".into(), "[g]".into(), "-loop".into(), loops.into()]);
            }
        }
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, (end - start).max(0.0), 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}
