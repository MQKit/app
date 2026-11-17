//! Compress video: by quality (CRF) or to fit a size (two passes, with a retry if the
//! file comes out over), with optional lower resolution, frame rate and trimming.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};

pub struct VideoCompress;

pub const VIDEO_INPUTS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "asf", "flv", "mpg", "mpeg", "vob", "ts", "mts", "m2ts", "3gp", "ogv",
];

/// The resolutions offered, as the short side (720 = 1280×720 in landscape).
pub fn resolution_options() -> Value {
    json!([ opt("keep", "Păstrează", "Keep"), opt(2160, "4K (2160p)", "4K (2160p)"), opt(1440, "1440p", "1440p"),
            opt(1080, "Full HD (1080p)", "Full HD (1080p)"), opt(720, "HD (720p)", "HD (720p)"),
            opt(480, "480p", "480p"), opt(360, "360p", "360p") ])
}

impl Plugin for VideoCompress {
    fn id(&self) -> &'static str {
        "video-compress"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Comprimare video", "Compress video"),
            "summary": t("Sub o mărime aleasă", "Under a size you choose"),
            "keywords": { "ro": ["micșorează video", "comprimă", "mai mic", "email", "whatsapp", "discord"], "en": ["shrink video", "compress", "smaller", "email", "whatsapp", "discord"] },
            "category": "video",
            "icon": "movie",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": VIDEO_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["mp4"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["durationSec", "hasVideo", "hasAudio", "width", "height"] },
            "ui": { "preview": "none", "estimate": true },
            "resources": { "class": "heavy", "diskFactor": 1.0 },
            "presets": [
                { "id": "balanced", "label": t("Echilibrat", "Balanced"), "default": true, "values": { "mode": "quality", "quality": 6, "maxResolution": "keep", "codec": "h264" } },
                { "id": "small", "label": t("Fișier mic", "Small file"), "values": { "mode": "quality", "quality": 4, "maxResolution": 1080, "codec": "h265" } },
                { "id": "discord", "label": t("Discord 10 MB", "Discord 10 MB"), "values": { "mode": "targetSize", "targetSize": 9700000, "codec": "h264" } },
                { "id": "email", "label": t("E-mail 25 MB", "Email 25 MB"), "values": { "mode": "targetSize", "targetSize": 22000000, "maxResolution": 720, "codec": "h264" } },
                { "id": "phone", "label": t("Telefon 720p", "Phone 720p"), "values": { "mode": "quality", "quality": 5, "maxResolution": 720, "maxFps": 30, "codec": "h264" } },
                { "id": "high", "label": t("Calitate înaltă", "High quality"), "values": { "mode": "quality", "quality": 8, "maxResolution": "keep", "codec": "h264", "speed": "slow" } }
            ],
            "groups": [ { "id": "cut", "label": t("Doar o parte", "Only a part"), "order": 1 }, { "id": "more", "label": t("Detalii", "Details"), "order": 2 } ],
            "fields": [
                { "key": "mode", "type": "select", "label": t("Cum comprimăm", "How to compress"), "default": "quality", "display": "chips",
                  "options": [ opt("quality", "După calitate", "By quality"), opt("targetSize", "Sub o mărime", "Under a size") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 10, "step": 1, "default": 6,
                  "help": t("Mai mică = fișier mai mic.", "Lower = smaller file."), "showIf": { "field": "mode", "eq": "quality" } },
                { "key": "targetSize", "type": "number", "label": t("Mărime maximă", "Maximum size"), "default": 25000000,
                  "min": 1000000, "max": 100000000000u64, "showIf": { "field": "mode", "eq": "targetSize" },
                  "units": { "base": "B", "display": [ { "id": "MB", "factor": 1000000 }, { "id": "GB", "factor": 1000000000 } ], "defaultDisplay": "MB" },
                  "suggestions": [ { "value": 9700000, "label": t("10 MB (Discord)", "10 MB (Discord)") }, { "value": 15500000, "label": t("16 MB (WhatsApp)", "16 MB (WhatsApp)") },
                                   { "value": 22000000, "label": t("25 MB (e-mail)", "25 MB (email)") }, { "value": 48500000, "label": t("50 MB", "50 MB") },
                                   { "value": 97000000, "label": t("100 MB", "100 MB") } ] },
                { "key": "maxResolution", "type": "select", "label": t("Rezoluție maximă", "Maximum resolution"), "default": "keep", "options": resolution_options() },
                { "key": "audio", "type": "select", "label": t("Sunet", "Sound"), "default": "keep", "display": "chips",
                  "options": [ opt("keep", "Păstrează", "Keep"), opt("remove", "Fără sunet", "No sound") ] },
                { "key": "speed", "type": "select", "label": t("Viteză", "Speed"), "default": "medium",
                  "options": [ opt("veryfast", "Rapid", "Fast"), opt("medium", "Echilibrat", "Balanced"), opt("slow", "Lent, puțin mai bun", "Slow, a little better") ] },
                { "key": "codec", "type": "select", "advanced": true, "group": "more", "default": "h264", "label": t("Codec", "Codec"),
                  "options": [ opt("h264", "H.264 (merge peste tot)", "H.264 (plays everywhere)"), opt("h265", "H.265 (mai mic, mai nou)", "H.265 (smaller, newer)") ] },
                { "key": "maxFps", "type": "select", "advanced": true, "group": "more", "default": "keep", "label": t("Cadre pe secundă", "Frames per second"),
                  "options": [ opt("keep", "Păstrează", "Keep"), opt(60, "60", "60"), opt(30, "30", "30"), opt(24, "24", "24") ] },
                { "key": "keepMetadata", "type": "toggle", "advanced": true, "group": "more", "default": true,
                  "label": t("Păstrează data și locația", "Keep the date and location") },
                { "key": "start", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("De la", "From"), "placeholder": t("început", "start") },
                { "key": "end", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("Până la", "To"), "placeholder": t("sfârșit", "end") }
            ],
            "rules": [
                { "id": "no_video", "when": { "file": "hasVideo", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au imagine și vor fi sărite.", "Some files have no picture and will be skipped.") },
                { "id": "h265_compat", "severity": "info", "when": { "field": "codec", "eq": "h265" },
                  "message": t("H.265 nu se redă pe unele calculatoare și televizoare mai vechi.", "H.265 doesn't play on some older computers and TVs.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_comprimat", "{name}_compressed") } },
            "errors": {
                "no_video": t("Fișierul nu are imagine.", "The file has no picture."),
                "target_too_small": t("Mărimea aleasă e prea mică pentru durata clipului. Alege o mărime mai mare sau taie clipul.",
                                      "The chosen size is too small for the clip's length. Pick a larger size or trim the clip."),
                "already_small": t("Fișierul e deja sub mărimea aleasă.", "The file is already under the chosen size."),
                "target_missed": t("Rezultatul e puțin peste mărimea aleasă.", "The result is a little over the chosen size.")
            }
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

    fn estimate(&self, req: &Request) -> Option<Value> {
        let mut input = 0u64;
        let mut output = 0.0;
        for f in &req.files {
            let size = file_size(f.input_path());
            input += size;
            let d = f.analysis.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
            let (s, e) = trim(req, d);
            let part = if d > 0.0 { (e - s) / d } else { 1.0 };
            output += if req.str_or("mode", "quality") == "targetSize" {
                (req.f64_or("targetSize", 25e6)).min(size as f64 * part)
            } else {
                // Rough: quality 6 ≈ 45 % of a phone video; each step ≈ ×1.25.
                let q = req.f64_or("quality", 6.0).clamp(1.0, 10.0);
                size as f64 * part * 0.45 * 1.25f64.powf(q - 6.0)
            };
        }
        Some(json!({ "inputBytes": input, "outputBytes": output.round(), "confidence": "low", "method": "formula" }))
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "encode", 1, 2);
        let info = if f.analysis.contains_key("durationSec") && f.analysis.contains_key("width") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        if info.get("hasVideo").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_video"));
        }
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let (start, end) = trim(req, duration);
        let length = (end - start).max(0.0);
        if duration > 0.0 && length <= 0.0 {
            return Err(FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "durationSec": duration })));
        }
        let has_audio = info.get("hasAudio").and_then(Value::as_bool).unwrap_or(false) && req.str_or("audio", "keep") != "remove";
        let out = ctx.file_dir(&f.id)?.join("result.mp4");
        let source = Source::of(&info);
        let hdr = ffmpeg::is_hdr(&info);

        if req.str_or("mode", "quality") == "targetSize" {
            let target = req.f64_or("targetSize", 25e6).max(1e5);
            if start == 0.0 && end >= duration && (in_bytes as f64) <= target {
                return Err(FileError::skipped("already_small", json!({ "bytes": in_bytes })));
            }
            if length <= 0.0 {
                return Err(FileError::new("target_too_small"));
            }
            let tracks = info.get("audioTracks").and_then(Value::as_array).map(|t| t.len()).unwrap_or(1).max(1) as f64;
            let audio_each = if target < 15e6 { 64.0 } else { 96.0 };
            let audio_k = if has_audio { audio_each * tracks } else { 0.0 };
            // 3 % below the target for the container.
            let total_k = target * 8.0 / 1000.0 / length * 0.97;
            let mut video_k = total_k - audio_k;
            if video_k < 60.0 {
                return Err(FileError::with("target_too_small", json!({ "videoKbps": video_k.round() })));
            }
            // Too few bits for the picture size: go down a step or more.
            let fps = source.fps.min(req.f64_or("maxFps", 1000.0));
            let mut short = cap_resolution(req, &source);
            for step in [1080.0, 720.0, 540.0, 480.0, 360.0] {
                if bits_per_pixel(video_k, short, &source, fps) >= 0.045 {
                    break;
                }
                if step < short {
                    short = step;
                }
            }
            let passlog = ctx.work_dir.join(format!("pass-{}", f.id));
            std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
            for attempt in 0..2 {
                let rate = format!("{}k", video_k.round() as u64);
                let mut a1 = head(start, end, duration, input);
                a1.extend(video_args(req, &source, short, fps, hdr));
                a1.extend(sv(&["-b:v", &rate, "-pass", "1", "-passlogfile"]));
                a1.push(passlog.to_string_lossy().into_owned());
                a1.extend(sv(&["-an", "-f", "null", "NUL"]));
                ffmpeg::run(ctx, &f.id, &a1, length, 1, 2)?;
                ctx.check_cancel()?;
                let mut a2 = head(start, end, duration, input);
                a2.extend(video_args(req, &source, short, fps, hdr));
                a2.extend(sv(&["-b:v", &rate, "-pass", "2", "-passlogfile"]));
                a2.push(passlog.to_string_lossy().into_owned());
                a2.extend(tail(req, has_audio, audio_each as u32, &info, &out));
                ffmpeg::run(ctx, &f.id, &a2, length, 2, 2)?;
                let got = file_size(&out) as f64;
                if got <= target || attempt == 1 {
                    if got > target {
                        ctx.out.warning(Some(&f.id), "target_missed", Some(json!({ "bytes": got })));
                    }
                    break;
                }
                video_k *= target / got * 0.95;
            }
        } else {
            let fps = source.fps.min(req.f64_or("maxFps", 1000.0));
            let short = cap_resolution(req, &source);
            let q = req.f64_or("quality", 6.0).clamp(1.0, 10.0);
            let crf = (35.0 - 2.0 * q).round() as i64 + if req.str_or("codec", "h264") == "h265" { 4 } else { 0 };
            let mut a = head(start, end, duration, input);
            a.extend(video_args(req, &source, short, fps, hdr));
            a.extend(sv(&["-crf", &crf.to_string()]));
            a.extend(tail(req, has_audio, 128, &info, &out));
            ffmpeg::run(ctx, &f.id, &a, length, 1, 1)?;
            // An already well-compressed clip can come out larger: nothing is written, as in
            // the other compress tools (unless it was trimmed, which is a different clip).
            let whole = start == 0.0 && end >= duration;
            let out_bytes = file_size(&out);
            if whole && out_bytes >= in_bytes {
                let _ = std::fs::remove_file(&out);
                return Err(FileError::skipped(codes::OUTPUT_LARGER, json!({ "inputBytes": in_bytes, "outputBytes": out_bytes })));
            }
        }
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, "mp4", json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Width, height (as stored) and frame rate of the first video track.
pub struct Source {
    pub w: f64,
    pub h: f64,
    pub fps: f64,
}

impl Source {
    pub fn of(info: &Map<String, Value>) -> Self {
        // `get`: indexing a Map panics on a missing key.
        let v = &info.get("video").cloned().unwrap_or(Value::Null);
        Source {
            w: v["width"].as_f64().unwrap_or(1280.0),
            h: v["height"].as_f64().unwrap_or(720.0),
            fps: v["fps"].as_f64().filter(|f| *f > 0.0 && *f < 1000.0).unwrap_or(30.0),
        }
    }
    pub fn short(&self) -> f64 {
        self.w.min(self.h)
    }
}

/// The short side asked for (never larger than the source).
pub fn cap_resolution(req: &Request, s: &Source) -> f64 {
    match req.opt_f64("maxResolution") {
        Some(r) if r > 0.0 => r.min(s.short()),
        _ => s.short(),
    }
}

fn bits_per_pixel(video_kbps: f64, short: f64, s: &Source, fps: f64) -> f64 {
    let scale = short / s.short();
    video_kbps * 1000.0 / (s.w * scale * s.h * scale * fps.max(1.0))
}

/// A time option, written as "1:30", "00:01:30.5" or a number of seconds.
pub fn time_opt(req: &Request, key: &str) -> Option<f64> {
    let v = req.options.get(key)?;
    v.as_str().and_then(ffmpeg::parse_time).or_else(|| v.as_f64())
}

pub fn trim(req: &Request, duration: f64) -> (f64, f64) {
    let start = time_opt(req, "start").unwrap_or(0.0).max(0.0);
    let end = time_opt(req, "end").filter(|e| *e > 0.0).unwrap_or(duration);
    (start, if duration > 0.0 { end.min(duration) } else { end })
}

/// Input with the cut, both as input options: `-ss` (fast seek) and `-t` count on the
/// source, so filters that change speed don't change where the cut falls.
pub fn head(start: f64, end: f64, duration: f64, input: &std::path::Path) -> Vec<String> {
    let mut a = Vec::new();
    if start > 0.0 {
        a.extend(sv(&["-ss", &format!("{start:.3}")]));
    }
    if end < duration || (duration == 0.0 && end > 0.0) {
        a.extend(sv(&["-t", &format!("{:.3}", end - start)]));
    }
    a.extend(["-i".to_string(), input.to_string_lossy().into_owned()]);
    a
}

/// Video encoder, scale and frame rate. The short side is scaled; `-2` keeps it even.
pub fn video_args(req: &Request, s: &Source, short: f64, fps: f64, hdr: bool) -> Vec<String> {
    let mut a = sv(&["-map", "0:v:0"]);
    let mut filters = Vec::new();
    if short < s.short() - 0.5 {
        let short = (short as i64 / 2) * 2;
        filters.push(if s.w >= s.h { format!("scale=-2:{short}:flags=bicubic") } else { format!("scale={short}:-2:flags=bicubic") });
    }
    if fps < s.fps - 0.01 {
        filters.push(format!("fps={fps}"));
    }
    filters.push(if hdr { ffmpeg::to_sdr("yuv420p") } else { "format=yuv420p".into() });
    a.extend(["-vf".into(), filters.join(",")]);
    if hdr {
        a.extend(ffmpeg::sdr_tags());
    }
    let preset = match req.str_or("speed", "medium") {
        "veryfast" => "veryfast",
        "slow" => "slow",
        _ => "medium",
    };
    if req.str_or("codec", "h264") == "h265" {
        a.extend(sv(&["-c:v", "libx265", "-preset", preset, "-tag:v", "hvc1", "-x265-params", "log-level=error"]));
    } else {
        a.extend(sv(&["-c:v", "libx264", "-preset", preset]));
    }
    a
}

/// The text subtitle tracks (an MP4 holds them as mov_text; picture subtitles from DVDs
/// and Blu-rays can't go in).
fn text_subtitles(info: &Map<String, Value>) -> Vec<usize> {
    info.get("subtitleTracks")
        .and_then(Value::as_array)
        .map(|t| {
            t.iter()
                .enumerate()
                .filter(|(_, s)| matches!(s["codec"].as_str(), Some("subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "text")))
                .map(|(i, _)| i)
                .collect()
        })
        .unwrap_or_default()
}

/// Audio (every track: a film's languages stay), text subtitles, metadata and the output file.
pub fn tail(req: &Request, audio: bool, kbps: u32, info: &Map<String, Value>, out: &std::path::Path) -> Vec<String> {
    let mut a = Vec::new();
    if audio {
        a.extend(sv(&["-map", "0:a?", "-c:a", "aac", "-b:a", &format!("{kbps}k")]));
    } else {
        a.push("-an".into());
    }
    let subs = text_subtitles(info);
    if subs.is_empty() {
        a.push("-sn".into());
    } else {
        for i in subs {
            a.extend(["-map".to_string(), format!("0:s:{i}")]);
        }
        a.extend(sv(&["-c:s", "mov_text"]));
    }
    a.push("-dn".into());
    if req.bool_or("keepMetadata", true) {
        a.extend(sv(&["-map_metadata", "0"]));
    } else {
        a.extend(sv(&["-map_metadata", "-1"]));
    }
    a.extend(sv(&["-movflags", "+faststart"]));
    a.push(out.to_string_lossy().into_owned());
    a
}
