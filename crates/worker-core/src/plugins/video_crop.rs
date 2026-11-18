//! Crop a video, or fit it to the shapes social networks use (9:16 for TikTok, Reels and
//! Shorts, 1:1, 4:5, 16:9): cut to the shape, or the whole picture over a blurred copy of
//! itself (or a colour). Speed and sound too. The preview shows one frame as it will be.

use super::video_compress::{trim, Source, VIDEO_INPUTS};
use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{ffmpeg, img};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct VideoCrop;

impl Plugin for VideoCrop {
    fn id(&self) -> &'static str {
        "video-crop"
    }

    fn manifest(&self) -> Value {
        let shaped = json!({ "field": "shape", "ne": "custom" });
        let custom = json!({ "field": "shape", "eq": "custom" });
        let cropping = json!({ "all": [ shaped, { "field": "fit", "eq": "crop" } ] });
        json!({
            "name": t("Decupare video pentru rețele", "Crop video for social media"),
            "summary": t("9:16, 1:1, 4:5, decupare", "9:16, 1:1, 4:5, crop"),
            "keywords": { "ro": ["decupează video", "tiktok", "reels", "shorts", "vertical", "pătrat"], "en": ["crop video", "tiktok", "reels", "shorts", "vertical", "square"] },
            "category": "video",
            "icon": "crop",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": VIDEO_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["mp4"],
                        "nameVariables": [ { "id": "shape", "label": t("forma", "shape") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasVideo", "hasAudio", "width", "height"] },
            "ui": { "preview": "result", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 1.2 },
            "presets": [
                { "id": "vertical", "label": t("TikTok, Reels, Shorts (9:16)", "TikTok, Reels, Shorts (9:16)"), "default": true, "values": { "shape": "9:16", "fit": "blur" } },
                { "id": "square", "label": t("Pătrat (1:1)", "Square (1:1)"), "values": { "shape": "1:1", "fit": "crop" } },
                { "id": "portrait", "label": t("Instagram (4:5)", "Instagram (4:5)"), "values": { "shape": "4:5", "fit": "crop" } },
                { "id": "wide", "label": t("YouTube (16:9)", "YouTube (16:9)"), "values": { "shape": "16:9", "fit": "blur" } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "shape", "type": "select", "label": t("Forma", "Shape"), "default": "9:16", "display": "chips",
                  "options": [ opt("9:16", "9:16 vertical", "9:16 vertical"), opt("1:1", "1:1 pătrat", "1:1 square"), opt("4:5", "4:5", "4:5"),
                               opt("16:9", "16:9 lat", "16:9 wide"), opt("custom", "Decupare liberă", "Free crop") ] },
                { "key": "fit", "type": "select", "label": t("Cum încape", "How it fits"), "default": "blur", "showIf": shaped,
                  "options": [ opt("crop", "Taie marginile", "Cut the edges"), opt("blur", "Toată imaginea, pe fundal estompat", "The whole picture, on a blurred background"),
                               opt("color", "Toată imaginea, cu benzi colorate", "The whole picture, with colored bars") ] },
                { "key": "focus", "type": "select", "label": t("Ce păstrezi", "What to keep"), "default": "center", "display": "chips", "showIf": cropping,
                  "options": [ opt("start", "Stânga / sus", "Left / top"), opt("center", "Mijlocul", "The middle"), opt("end", "Dreapta / jos", "Right / bottom") ] },
                { "key": "barColor", "type": "color", "label": t("Culoarea benzilor", "Bar color"), "default": "#000000",
                  "showIf": { "all": [ shaped, { "field": "fit", "eq": "color" } ] } },
                { "key": "cropLeft", "type": "range", "label": t("Taie din stânga", "Cut from the left"), "min": 0, "max": 45, "step": 1, "unit": "%", "default": 0, "showIf": custom },
                { "key": "cropRight", "type": "range", "label": t("Taie din dreapta", "Cut from the right"), "min": 0, "max": 45, "step": 1, "unit": "%", "default": 0, "showIf": custom },
                { "key": "cropTop", "type": "range", "label": t("Taie de sus", "Cut from the top"), "min": 0, "max": 45, "step": 1, "unit": "%", "default": 0, "showIf": custom },
                { "key": "cropBottom", "type": "range", "label": t("Taie de jos", "Cut from the bottom"), "min": 0, "max": 45, "step": 1, "unit": "%", "default": 0, "showIf": custom },
                { "key": "resolution", "type": "select", "label": t("Rezoluție", "Resolution"), "default": 1080,
                  "options": [ opt(2160, "4K", "4K"), opt(1080, "Full HD (1080)", "Full HD (1080)"), opt(720, "HD (720)", "HD (720)"), opt(480, "480", "480") ],
                  "help": t("Latura scurtă. Niciodată mai mare decât originalul.", "The short side. Never larger than the original.") },
                { "key": "speed", "type": "select", "advanced": true, "group": "more", "default": 1, "label": t("Viteză", "Speed"),
                  "options": [ opt(0.5, "0,5× (încetinit)", "0.5× (slow motion)"), opt(0.75, "0,75×", "0.75×"), opt(1, "Normală", "Normal"),
                               opt(1.25, "1,25×", "1.25×"), opt(1.5, "1,5×", "1.5×"), opt(2, "2×", "2×"), opt(4, "4×", "4×") ] },
                { "key": "sound", "type": "select", "advanced": true, "group": "more", "default": "keep", "display": "chips", "label": t("Sunet", "Sound"),
                  "options": [ opt("keep", "Păstrează", "Keep"), opt("mute", "Fără sunet", "No sound") ] },
                { "key": "start", "type": "time", "advanced": true, "group": "more", "default": "", "label": t("De la", "From"), "placeholder": t("început", "start") },
                { "key": "end", "type": "time", "advanced": true, "group": "more", "default": "", "label": t("Până la", "To"), "placeholder": t("sfârșit", "end") }
            ],
            "rules": [
                { "id": "no_video", "when": { "file": "hasVideo", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au imagine și vor fi sărite.", "Some files have no picture and will be skipped.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_{shape}", "{name}_{shape}") } },
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

    /// One frame, a third of the way in, as the result will show it.
    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        let Some(f) = super::preview_file(req) else { return Ok(None) };
        let info = if f.analysis.contains_key("video") { f.analysis.clone() } else { ffmpeg::probe(req, f.input_path())? };
        if info.get("hasVideo").and_then(Value::as_bool) == Some(false) {
            return Ok(None);
        }
        let max = req.target.as_ref().and_then(|t| t.get("maxPx")).and_then(Value::as_u64).unwrap_or(480).clamp(64, 1600) as f64;
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let (start, end) = trim(req, duration);
        let at = start + (end - start).max(0.0) / 3.0;
        let plan = Plan::of(req, &Source::of(&info));
        // Small: the longer side at most the preview box.
        let k = (max / plan.out_w.max(plan.out_h)).min(1.0);
        let filter = format!("{},scale={}:{},format=rgb24", plan.filter(), even(plan.out_w * k), even(plan.out_h * k));
        let dir = req.preview_dir.clone().ok_or_else(|| FileError::new(codes::INVALID_OPTIONS))?;
        std::fs::create_dir_all(&dir).map_err(super::io_err)?;
        let png = dir.join(format!("{}-frame.png", req.id.clone().unwrap_or_else(|| "p".into())));
        let mut child = ffmpeg::command(&ffmpeg::exe(req, "ffmpeg")?)
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-ss", &format!("{at:.3}"), "-i"])
            .arg(f.input_path())
            .args(["-frames:v", "1", "-filter_complex", &filter, "-f", "image2"])
            .arg(&png)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "ffmpeg", "message": e.to_string() })))?;
        let (status, _, err) = crate::common::proc::output(&mut child, "ffmpeg", std::time::Duration::from_secs(25))?;
        if !status.success() {
            return Err(FileError::engine(String::from_utf8_lossy(&err)));
        }
        let data = std::fs::read(&png).map_err(super::io_err)?;
        crate::common::preview::write_single(req, &data, "png").map(Some)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "encode", 1, 1);
        let info = if f.analysis.contains_key("video") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        if info.get("hasVideo").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_video"));
        }
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let (start, end) = trim(req, duration);
        let plan = Plan::of(req, &Source::of(&info));
        let speed = req.f64_or("speed", 1.0).clamp(0.25, 4.0);
        let hdr = ffmpeg::is_hdr(&info);

        let mut vf = format!("{},scale={}:{}", plan.filter(), even(plan.out_w), even(plan.out_h));
        if (speed - 1.0).abs() > 0.001 {
            vf.push_str(&format!(",setpts=PTS/{speed}"));
        }
        vf.push(',');
        vf.push_str(&if hdr { ffmpeg::to_sdr("yuv420p") } else { "format=yuv420p".into() });
        vf.push_str("[v]");

        let mut a = super::video_compress::head(start, end, duration, input);
        a.extend(sv(&["-filter_complex", &vf, "-map", "[v]"]));
        let with_sound = req.str_or("sound", "keep") == "keep" && info.get("hasAudio").and_then(Value::as_bool).unwrap_or(false);
        if with_sound {
            a.extend(sv(&["-map", "0:a:0"]));
            if (speed - 1.0).abs() > 0.001 {
                a.extend(sv(&["-af", &atempo(speed)]));
            }
            a.extend(sv(&["-c:a", "aac", "-b:a", "160k"]));
        } else {
            a.push("-an".into());
        }
        a.extend(sv(&["-c:v", "libx264", "-preset", "medium", "-crf", "20", "-profile:v", "high", "-sn", "-dn",
                      "-map_metadata", "0", "-movflags", "+faststart"]));
        if hdr {
            a.extend(ffmpeg::sdr_tags());
        }
        let out = ctx.file_dir(&f.id)?.join("result.mp4");
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, ((end - start) / speed).max(0.0), 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        let shape = req.str_or("shape", "9:16").replace(':', "x");
        ctx.emit_output(&f.id, &out, "mp4", json!({ "shape": if shape == "custom" { "crop".to_string() } else { shape } }), in_bytes);
        Ok(Value::Null)
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn even(v: f64) -> i64 {
    ((v / 2.0).round() as i64 * 2).max(2)
}

/// Speeds beyond what one atempo does (0.5–2) as a chain of them.
fn atempo(speed: f64) -> String {
    let mut parts = Vec::new();
    let mut s = speed;
    while s > 2.0 {
        parts.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s < 0.5 {
        parts.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    parts.push(format!("atempo={s:.4}"));
    parts.join(",")
}

/// What to do with the picture: the filter (from [0:v]) and the size it ends at.
struct Plan {
    graph: String,
    out_w: f64,
    out_h: f64,
}

impl Plan {
    fn of(req: &Request, s: &Source) -> Plan {
        let (w, h) = (s.w.max(2.0), s.h.max(2.0));
        let res = req.f64_or("resolution", 1080.0).max(144.0);
        // The short side wanted, never more than the picture has.
        let fit_short = |ow: f64, oh: f64| {
            let k = (res / ow.min(oh)).min(1.0);
            (ow * k, oh * k)
        };
        let shape = req.str_or("shape", "9:16");
        if shape == "custom" {
            let p = |k: &str| (req.f64_or(k, 0.0).clamp(0.0, 45.0)) / 100.0;
            let (l, r, t, b) = (p("cropLeft"), p("cropRight"), p("cropTop"), p("cropBottom"));
            let (cw, ch) = (w * (1.0 - l - r).max(0.1), h * (1.0 - t - b).max(0.1));
            let (ow, oh) = fit_short(cw, ch);
            return Plan { graph: format!("[0:v]crop={}:{}:{}:{}", even(cw), even(ch), (w * l).round() as i64, (h * t).round() as i64), out_w: ow, out_h: oh };
        }
        let (aw, ah) = shape.split_once(':').and_then(|(a, b)| Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?))).unwrap_or((9.0, 16.0));
        let ar = aw / ah;
        match req.str_or("fit", "blur") {
            "crop" => {
                let (cw, ch) = if w / h > ar { (h * ar, h) } else { (w, w / ar) };
                let k = match req.str_or("focus", "center") { "start" => 0.0, "end" => 1.0, _ => 0.5 };
                let (x, y) = ((w - cw) * k, (h - ch) * k);
                let (ow, oh) = fit_short(cw, ch);
                Plan { graph: format!("[0:v]crop={}:{}:{}:{}", even(cw), even(ch), x.round() as i64, y.round() as i64), out_w: ow, out_h: oh }
            }
            fit => {
                // The frame holds the whole picture on its long side.
                let (fw, fh) = if w / h > ar { (w, w / ar) } else { (h * ar, h) };
                let (ow, oh) = fit_short(fw, fh);
                let (ow, oh) = (even(ow), even(oh));
                let fg = format!("scale={ow}:{oh}:force_original_aspect_ratio=decrease:force_divisible_by=2");
                let graph = if fit == "color" {
                    let c = img::parse_color(req.str_or("barColor", "#000000"));
                    format!("[0:v]{fg},pad={ow}:{oh}:(ow-iw)/2:(oh-ih)/2:color=0x{:02x}{:02x}{:02x}", c[0], c[1], c[2])
                } else {
                    // The same picture, filling the frame and blurred, under the whole one.
                    format!("[0:v]split=2[bg0][fg0];[bg0]scale={ow}:{oh}:force_original_aspect_ratio=increase,crop={ow}:{oh},boxblur=luma_radius=min(h\\,w)/20:luma_power=2,eq=brightness=-0.06[bg];[fg0]{fg}[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2")
                };
                Plan { graph, out_w: ow as f64, out_h: oh as f64 }
            }
        }
    }

    /// The graph, ending in a label-less output (more filters are appended after a comma).
    fn filter(&self) -> String {
        self.graph.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn req(opts: Value) -> Request {
        serde_json::from_value(json!({ "cmd": "run", "options": opts })).unwrap()
    }
    fn src(w: f64, h: f64) -> Source {
        Source::of(&serde_json::from_value::<Map<String, Value>>(json!({ "video": { "width": w, "height": h, "fps": 30 } })).unwrap())
    }

    #[test]
    fn shapes_and_sizes() {
        // Landscape 1920×1080 cut to 9:16: a 608×1080 strip in the middle.
        let p = Plan::of(&req(json!({ "shape": "9:16", "fit": "crop" })), &src(1920.0, 1080.0));
        assert_eq!(p.graph, "[0:v]crop=608:1080:656:0");
        assert_eq!((even(p.out_w), even(p.out_h)), (608, 1080));
        // The whole picture in a 9:16 frame: 1080 wide, 1920 tall, blurred behind.
        let p = Plan::of(&req(json!({ "shape": "9:16", "fit": "blur" })), &src(1920.0, 1080.0));
        assert_eq!((p.out_w, p.out_h), (1080.0, 1920.0));
        assert!(p.graph.contains("boxblur") && p.graph.contains("overlay"));
        // 4K source, 720 asked: the short side is 720.
        let p = Plan::of(&req(json!({ "shape": "1:1", "fit": "crop", "resolution": 720 })), &src(3840.0, 2160.0));
        assert_eq!((even(p.out_w), even(p.out_h)), (720, 720));
        // Never enlarged.
        let p = Plan::of(&req(json!({ "shape": "16:9", "fit": "color", "resolution": 2160 })), &src(640.0, 360.0));
        assert_eq!((p.out_w, p.out_h), (640.0, 360.0));
        assert!(p.graph.contains("color=0x000000"));
        // Free crop: 10 % off each side.
        let p = Plan::of(&req(json!({ "shape": "custom", "cropLeft": 10, "cropRight": 10, "cropTop": 10, "cropBottom": 10 })), &src(1000.0, 500.0));
        assert_eq!(p.graph, "[0:v]crop=800:400:100:50");
    }

    #[test]
    fn tempo_chain() {
        assert_eq!(atempo(1.5), "atempo=1.5000");
        assert_eq!(atempo(4.0), "atempo=2,atempo=2.0000");
        assert_eq!(atempo(0.25), "atempo=0.5,atempo=0.5000");
    }
}
