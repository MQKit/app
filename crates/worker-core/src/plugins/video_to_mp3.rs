//! Tool 10: video to MP3 (and other audio formats), with FFmpeg. Audio codecs only,
//! so it does not depend on the H.264/HEVC decision.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct VideoToMp3;

const INPUTS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v", "3gp", "mpg", "mpeg", "ts", "mts", "m2ts", "vob", "ogv",
    "wav", "m4a", "aac", "flac", "ogg", "opus", "wma", "amr", "mp3", "aiff",
];

impl Plugin for VideoToMp3 {
    fn id(&self) -> &'static str {
        "video-to-mp3"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Video în MP3", "Video to MP3"),
            "summary": t("Scoate sunetul", "Extract the sound"),
            "keywords": { "ro": ["audio", "sunet", "muzică", "mp3", "extrage", "melodie"], "en": ["audio", "sound", "music", "mp3", "extract", "song"] },
            "category": "audio",
            "icon": "music",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["mp3", "m4a", "wav", "flac", "ogg", "opus"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasVideo", "hasAudio", "audioTracks"] },
            "ui": { "preview": "none", "estimate": true },
            "resources": { "class": "heavy", "diskFactor": 0.5 },
            "presets": [],
            "groups": [ { "id": "cut", "label": t("Doar o parte", "Only a part"), "order": 1 }, { "id": "more", "label": t("Detalii", "Details"), "order": 2 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "mp3", "display": "chips",
                  "options": [ opt("mp3", "MP3", "MP3"), opt("m4a", "M4A", "M4A"), opt("wav", "WAV", "WAV"),
                               opt("flac", "FLAC", "FLAC"), opt("ogg", "OGG", "OGG"), opt("opus", "Opus", "Opus") ] },
                { "key": "bitrate", "type": "select", "label": t("Calitate", "Quality"), "default": 192,
                  "showIf": { "field": "format", "in": ["mp3", "m4a", "ogg", "opus"] },
                  "options": [ opt(128, "Bună (128 kbps)", "Good (128 kbps)"), opt(192, "Foarte bună (192 kbps)", "Very good (192 kbps)"),
                               opt(256, "Excelentă (256 kbps)", "Excellent (256 kbps)"), opt(320, "Maximă (320 kbps)", "Maximum (320 kbps)") ] },
                { "key": "audioTrack", "type": "select", "label": t("Pista audio", "Audio track"), "default": 0,
                  "optionsFrom": { "file": "audioTracks", "combine": "single" },
                  "showIf": { "file": "audioTracks.count", "gt": 1 } },
                { "key": "start", "type": "time", "advanced": true, "group": "cut", "default": "",
                  "label": t("De la", "From"), "placeholder": t("început", "start") },
                { "key": "end", "type": "time", "advanced": true, "group": "cut", "default": "",
                  "label": t("Până la", "To"), "placeholder": t("sfârșit", "end") },
                { "key": "normalize", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Volum egal (normalizare)", "Even volume (normalize)"),
                  "help": t("Aduce sunetul la un volum obișnuit, ca la radio.", "Brings the sound to a standard loudness.") },
                { "key": "mono", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Mono (fișier mai mic)", "Mono (smaller file)") }
            ],
            "rules": [
                { "id": "no_audio", "when": { "file": "hasAudio", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au sunet și vor fi sărite.", "Some files have no sound and will be skipped.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "no_audio": t("Fișierul nu are sunet.", "The file has no sound.")
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
        let format = req.str_or("format", "mp3");
        let kbps = match format {
            "wav" => 1411.0,
            "flac" => 800.0,
            _ => req.f64_or("bitrate", 192.0),
        };
        let mut total = 0.0;
        let mut input = 0u64;
        for f in &req.files {
            input += file_size(f.input_path());
            let d = f.analysis.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
            let (s, e) = range(req, d);
            total += (e - s).max(0.0) * kbps * 1000.0 / 8.0;
        }
        Some(json!({ "inputBytes": input, "outputBytes": total.round(), "confidence": if format == "flac" { "low" } else { "high" }, "method": "formula" }))
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let input = f.input_path();
        let in_bytes = file_size(input);
        let info = if f.analysis.contains_key("durationSec") { f.analysis.clone() } else { ffmpeg::probe(ctx.req, input)? };
        if info.get("hasAudio").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_audio"));
        }
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let (start, end) = range(ctx.req, duration);
        if duration > 0.0 && (start >= duration || end <= start) {
            return Err(FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "durationSec": duration })));
        }
        let format = ctx.req.str_or("format", "mp3");
        let kbps = ctx.req.f64_or("bitrate", 192.0) as u32;
        let track = ctx.req.f64_or("audioTrack", 0.0) as u32;
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));

        let mut args: Vec<String> = Vec::new();
        if start > 0.0 {
            args.extend(["-ss".into(), format!("{start:.3}")]);
        }
        args.extend(["-i".into(), input.to_string_lossy().into_owned()]);
        if end < duration || duration == 0.0 && end > 0.0 {
            args.extend(["-t".into(), format!("{:.3}", end - start)]);
        }
        // A song's cover (an audio file with a picture) goes along where the format holds one.
        let cover = info.get("hasAttachedPic").and_then(Value::as_bool) == Some(true)
            && info.get("hasVideo").and_then(Value::as_bool) != Some(true)
            && matches!(format, "mp3" | "m4a" | "flac");
        args.extend(["-map".into(), format!("0:a:{track}")]);
        if cover {
            args.extend(["-map".into(), "0:v:0".into(), "-c:v".into(), "copy".into(), "-disposition:v:0".into(), "attached_pic".into()]);
        } else {
            args.push("-vn".into());
        }
        args.extend(["-sn".into(), "-dn".into()]);
        args.extend(["-map_metadata".into(), "0".into()]);
        let mut filters = Vec::new();
        if ctx.req.bool_or("normalize", false) {
            filters.push("loudnorm=I=-16:TP=-1.5:LRA=11".to_string());
        }
        if !filters.is_empty() {
            args.extend(["-af".into(), filters.join(",")]);
        }
        if ctx.req.bool_or("mono", false) {
            args.extend(["-ac".into(), "1".into()]);
        }
        let codec: Vec<String> = match format {
            "mp3" => vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), format!("{kbps}k"), "-id3v2_version".into(), "3".into()],
            "m4a" => vec!["-c:a".into(), "aac".into(), "-b:a".into(), format!("{kbps}k"), "-movflags".into(), "+faststart".into()],
            "wav" => vec!["-c:a".into(), "pcm_s16le".into()],
            "flac" => vec!["-c:a".into(), "flac".into()],
            "ogg" => vec!["-c:a".into(), "libvorbis".into(), "-b:a".into(), format!("{kbps}k")],
            "opus" => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), format!("{}k", kbps.min(256))],
            other => return Err(FileError::with(codes::INVALID_OPTIONS, json!({ "format": other }))),
        };
        args.extend(codec);
        args.push(out.to_string_lossy().into_owned());
        ctx.progress(&f.id, 0.0, "encode", 1, 1);
        ffmpeg::run(ctx, &f.id, &args, end - start, 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(json!({ "track": track }))
    }
}

fn range(req: &Request, duration: f64) -> (f64, f64) {
    let start = req.opt_str("start").and_then(ffmpeg::parse_time).unwrap_or(0.0).max(0.0);
    let end = req.opt_str("end").and_then(ffmpeg::parse_time).filter(|e| *e > 0.0).unwrap_or(duration);
    (start, if duration > 0.0 { end.min(duration) } else { end })
}
