//! Merge videos (or audio files) into one, in the chosen order. Same format: joined by
//! copying, instantly. Different formats: re-encoded to match the first clip.

use super::video_compress::VIDEO_INPUTS;
use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};

pub struct MediaMerge;

const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "aiff"];

impl Plugin for MediaMerge {
    fn id(&self) -> &'static str {
        "media-merge"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = VIDEO_INPUTS.iter().chain(AUDIO.iter()).copied().collect();
        json!({
            "name": t("Unire video și audio", "Merge video and audio"),
            "summary": t("Mai multe clipuri într-unul", "Many clips into one"),
            "keywords": { "ro": ["unește", "lipește", "combină", "clipuri", "melodii"], "en": ["join", "combine", "merge", "clips", "songs"] },
            "category": "video",
            "icon": "files",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "combined", "ordered": true, "minFiles": 2 },
            "output": { "kind": "single", "extensions": ["mp4", "mkv", "mp3", "m4a", "wav"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasVideo", "hasAudio", "width", "height"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "standard" },
            "resources": { "class": "heavy", "diskFactor": 1.2 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "method", "type": "select", "label": t("Cum unesc", "How to join"), "default": "auto",
                  "options": [ opt("auto", "Automat (copiază dacă se poate)", "Automatic (copies when it can)"),
                               opt("reencode", "Recodează tot (pentru clipuri diferite)", "Re-encode everything (for different clips)") ] },
                { "key": "audioFormat", "type": "select", "label": t("Format, dacă sunt doar sunete", "Format, for sound only"), "default": "mp3", "display": "chips",
                  "options": [ opt("mp3", "MP3", "MP3"), opt("m4a", "M4A", "M4A"), opt("wav", "WAV", "WAV") ] }
            ],
            "rules": [
                { "id": "mixed", "severity": "info", "when": { "file": "hasVideo", "eq": false, "quant": "any" },
                  "message": t("Fișierele fără imagine apar pe ecran negru în video.", "Files without a picture show a black screen in the video.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_unit", "{name}_merged") } },
            "errors": {}
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

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        ctx.run_one("*", || merge(ctx));
        Ok(())
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn merge(ctx: &Ctx) -> Result<Value, FileError> {
    let req = ctx.req;
    ctx.progress("*", 0.0, "encode", 1, 1);
    let mut infos: Vec<Map<String, Value>> = Vec::new();
    let mut in_bytes = 0;
    for f in &req.files {
        in_bytes += file_size(f.input_path());
        infos.push(if f.analysis.contains_key("durationSec") && f.analysis.contains_key("audioTracks") { f.analysis.clone() } else { ffmpeg::probe(req, f.input_path())? });
    }
    let total: f64 = infos.iter().map(|i| i.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0)).sum();
    let video = infos.iter().any(|i| i.get("hasVideo").and_then(Value::as_bool) == Some(true));
    let first_ext = req.files[0].input_path().extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();

    // Same codecs and picture size everywhere: join by copying.
    // `get`, not indexing: a Map panics on a missing key (audio files have no "video").
    // Copying only works when every stream matches exactly: a different sample rate or
    // frame rate copied together plays at the wrong speed.
    let key = |i: &Map<String, Value>| {
        let v = i.get("video").cloned().unwrap_or(Value::Null);
        let a = i.get("audioTracks").cloned().unwrap_or(Value::Null);
        let tracks = a.as_array().map(|t| t.len()).unwrap_or(0);
        format!(
            "{}|{:?}|{:?}|{:?}|{}|{:?}|{}|{:?}|{:?}|{}",
            v["codec"].as_str().unwrap_or(""),
            v["width"].as_i64(),
            v["height"].as_i64(),
            v["fps"].as_f64(),
            v["pixFmt"].as_str().unwrap_or(""),
            v["rotation"].as_f64(),
            a[0]["codec"].as_str().unwrap_or(""),
            a[0]["sampleRate"].as_i64(),
            a[0]["channels"].as_i64(),
            tracks
        )
    };
    let same = infos.iter().all(|i| key(i) == key(&infos[0]));
    let ext = if video {
        if same && req.str_or("method", "auto") == "auto" && matches!(first_ext.as_str(), "mp4" | "mkv" | "mov" | "webm") { first_ext.clone() } else { "mp4".into() }
    } else {
        req.str_or("audioFormat", "mp3").to_string()
    };
    let out = ctx.file_dir("merged")?.join(format!("result.{ext}"));

    let copy = same && req.str_or("method", "auto") == "auto" && (video || ext == first_ext);
    let mut a = Vec::new();
    if copy {
        std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
        let list = ctx.work_dir.join("concat.txt");
        let body: String = req.files.iter().map(|f| format!("file '{}'\n", f.input_path().to_string_lossy().replace('\'', "'\\''"))).collect();
        std::fs::write(&list, body).map_err(super::io_err)?;
        a.extend(sv(&["-f", "concat", "-safe", "0", "-i"]));
        a.push(list.to_string_lossy().into_owned());
        a.extend(sv(&["-map", "0:v?", "-map", "0:a?", "-c", "copy"]));
    } else {
        let first_v = infos.iter().find(|i| i.get("hasVideo").and_then(Value::as_bool) == Some(true));
        let even = |x: i64| (x / 2 * 2).max(2);
        let fv = first_v.and_then(|i| i.get("video")).cloned().unwrap_or(Value::Null);
        let w = even(fv["width"].as_i64().unwrap_or(1280));
        let h = even(fv["height"].as_i64().unwrap_or(720));
        let fps = fv["fps"].as_f64().filter(|f| *f > 0.0 && *f <= 120.0).unwrap_or(30.0);
        let mut fc = String::new();
        let mut parts = String::new();
        for (n, f) in req.files.iter().enumerate() {
            a.extend(["-i".into(), f.input_path().to_string_lossy().into_owned()]);
            let info = &infos[n];
            let d = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0).max(0.1);
            if video {
                if info.get("hasVideo").and_then(Value::as_bool) == Some(true) {
                    // An HDR clip among ordinary ones: tone mapped, or it shows grey.
                    let sdr = if ffmpeg::is_hdr(info) { format!("{},", ffmpeg::to_sdr("yuv420p")) } else { String::new() };
                    fc += &format!("[{n}:v:0]{sdr}scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p[v{n}];");
                } else {
                    fc += &format!("color=c=black:s={w}x{h}:r={fps}:d={d:.3},format=yuv420p[v{n}];");
                }
                parts += &format!("[v{n}]");
            }
            if info.get("hasAudio").and_then(Value::as_bool) == Some(true) {
                fc += &format!("[{n}:a:0]aresample=48000,aformat=channel_layouts=stereo[a{n}];");
            } else {
                fc += &format!("anullsrc=r=48000:cl=stereo,atrim=duration={d:.3}[a{n}];");
            }
            parts += &format!("[a{n}]");
        }
        let count = req.files.len();
        fc += &format!("{parts}concat=n={count}:v={}:a=1", video as u8);
        fc += if video { "[v][a]" } else { "[a]" };
        a.extend(["-filter_complex".into(), fc]);
        if video {
            a.extend(sv(&["-map", "[v]", "-c:v", "libx264", "-preset", "medium", "-crf", "20"]));
        }
        a.extend(sv(&["-map", "[a]"]));
        a.extend(match ext.as_str() {
            "wav" => sv(&["-c:a", "pcm_s16le"]),
            "mp3" => sv(&["-c:a", "libmp3lame", "-b:a", "192k"]),
            _ => sv(&["-c:a", "aac", "-b:a", "160k"]),
        });
    }
    if matches!(ext.as_str(), "mp4" | "m4a" | "mov") {
        a.extend(sv(&["-movflags", "+faststart"]));
    }
    a.push(out.to_string_lossy().into_owned());
    ffmpeg::run(ctx, "*", &a, total, 1, 1)?;
    ctx.progress("*", 1.0, "encode", 1, 1);
    ctx.emit_output("*", &out, &ext, json!({ "files": req.files.len() }), in_bytes);
    Ok(json!({ "copied": copy }))
}
