//! Trim video and audio: keep a part or cut a part out, exactly (re-encoded) or fast
//! (copied, from the nearest keyframe). The result keeps the original format.

use super::video_compress::{time_opt, VIDEO_INPUTS};
use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct MediaTrim;

const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "aiff"];

impl Plugin for MediaTrim {
    fn id(&self) -> &'static str {
        "media-trim"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = VIDEO_INPUTS.iter().chain(AUDIO.iter()).copied().collect();
        json!({
            "name": t("Tăiere video și audio", "Trim video and audio"),
            "summary": t("Păstrează doar o parte", "Keep just a part"),
            "keywords": { "ro": ["taie", "scurtează", "decupează", "început", "sfârșit"], "en": ["trim", "cut", "shorten", "clip", "start", "end"] },
            "category": "video",
            "icon": "scissors",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": inputs, "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasVideo", "hasAudio"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 1.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "action", "type": "select", "label": t("Ce faci", "What to do"), "default": "keep", "display": "chips",
                  "options": [ opt("keep", "Păstrează partea aleasă", "Keep the chosen part"), opt("remove", "Scoate partea aleasă", "Remove the chosen part") ] },
                { "key": "start", "type": "time", "label": t("De la", "From"), "default": "", "placeholder": t("început", "start"),
                  "help": t("De exemplu 1:30 sau 0:01:30.5", "For example 1:30 or 0:01:30.5") },
                { "key": "end", "type": "time", "label": t("Până la", "To"), "default": "", "placeholder": t("sfârșit", "end") },
                { "key": "precision", "type": "select", "label": t("Precizie", "Precision"), "default": "exact",
                  "options": [ opt("exact", "Exactă (recodează)", "Exact (re-encodes)"), opt("fast", "Rapidă, fără pierderi (de la cel mai apropiat cadru-cheie)", "Fast, lossless (from the nearest keyframe)") ],
                  "showIf": { "field": "action", "eq": "keep" } }
            ],
            "rules": [
                { "id": "choose_times", "severity": "error", "fields": ["start", "end"],
                  "when": { "all": [ { "field": "start", "eq": "" }, { "field": "end", "eq": "" } ] },
                  "message": t("Alege de unde începe sau unde se termină.", "Choose where it starts or where it ends.") },
                { "id": "fast_keyframe", "severity": "info", "when": { "all": [ { "field": "action", "eq": "keep" }, { "field": "precision", "eq": "fast" } ] },
                  "message": t("Tăierea rapidă poate începe cu o secundă-două mai devreme.", "A fast cut may start a second or two early.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_taiat", "{name}_trimmed") } },
            "errors": {
                "nothing_chosen": t("Alege de unde și până unde.", "Choose where it starts and ends."),
                "no_media": t("Fișierul nu are nici imagine, nici sunet.", "The file has neither picture nor sound.")
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

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "encode", 1, 1);
        let info = if f.analysis.contains_key("durationSec") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let has_v = info.get("hasVideo").and_then(Value::as_bool).unwrap_or(false);
        let has_a = info.get("hasAudio").and_then(Value::as_bool).unwrap_or(false);
        if !has_v && !has_a {
            return Err(FileError::new("no_media"));
        }
        let start = time_opt(req, "start").unwrap_or(0.0).max(0.0);
        let end = time_opt(req, "end").filter(|e| *e > 0.0).unwrap_or(duration);
        let end = if duration > 0.0 { end.min(duration) } else { end };
        if end <= start || (start == 0.0 && (end - duration).abs() < 0.01) {
            return Err(FileError::new("nothing_chosen"));
        }
        let ext = input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp4".into());
        let out = ctx.file_dir(&f.id)?.join(format!("result.{ext}"));
        let mut a: Vec<String> = Vec::new();
        let expected;
        if req.str_or("action", "keep") == "remove" {
            // Two pieces, joined: [0, start) and [end, duration); every audio track (a film's languages).
            let tracks = if has_a { info.get("audioTracks").and_then(Value::as_array).map(|t| t.len()).unwrap_or(1).max(1) } else { 0 };
            let mut fc = String::new();
            let mut parts = String::new();
            if has_v {
                fc += &format!("[0:v:0]trim=0:{start:.3},setpts=PTS-STARTPTS[v0];[0:v:0]trim=start={end:.3},setpts=PTS-STARTPTS[v1];");
            }
            for t in 0..tracks {
                fc += &format!("[0:a:{t}]atrim=0:{start:.3},asetpts=PTS-STARTPTS[a0_{t}];[0:a:{t}]atrim=start={end:.3},asetpts=PTS-STARTPTS[a1_{t}];");
            }
            for i in 0..2 {
                if has_v {
                    parts += &format!("[v{i}]");
                }
                for t in 0..tracks {
                    parts += &format!("[a{i}_{t}]");
                }
            }
            fc += &format!("{parts}concat=n=2:v={}:a={tracks}", has_v as u8);
            if has_v {
                fc += "[v]";
            }
            for t in 0..tracks {
                fc += &format!("[a{t}]");
            }
            a.extend(["-i".into(), input.to_string_lossy().into_owned(), "-filter_complex".into(), fc]);
            if has_v {
                a.extend(sv(&["-map", "[v]"]));
            }
            for t in 0..tracks {
                a.extend(["-map".to_string(), format!("[a{t}]")]);
            }
            a.extend(codecs(&ext, has_v, has_a, &info));
            expected = (duration - (end - start)).max(0.0);
        } else {
            a.extend(sv(&["-ss", &format!("{start:.3}"), "-t", &format!("{:.3}", end - start)]));
            a.extend(["-i".into(), input.to_string_lossy().into_owned()]);
            let cover = !has_v && info.get("hasAttachedPic").and_then(Value::as_bool) == Some(true);
            if cover {
                // A song's cover picture.
                a.extend(sv(&["-map", "0:a?", "-map", "0:v:0", "-c:v", "copy", "-disposition:v:0", "attached_pic"]));
            } else {
                a.extend(sv(&["-map", "0:v:0?", "-map", "0:a?"]));
            }
            // Subtitles cut along, into the same format they came from.
            if matches!(ext.as_str(), "mkv" | "mp4" | "m4v" | "mov" | "webm") {
                a.extend(sv(&["-map", "0:s?", "-c:s", "copy"]));
            }
            if req.str_or("precision", "exact") == "fast" {
                a.extend(sv(&["-c:v", "copy", "-c:a", "copy", "-avoid_negative_ts", "make_zero"]));
            } else {
                a.extend(codecs(&ext, has_v && !cover, has_a, &info));
            }
            expected = end - start;
            // On the output too: a subtitle running past the cut would make the file longer.
            a.extend(sv(&["-t", &format!("{:.3}", end - start)]));
        }
        a.extend(sv(&["-map_metadata", "0"]));
        if matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "m4a") {
            a.extend(sv(&["-movflags", "+faststart"]));
        }
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, expected, 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, &ext, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Encoders that keep the file's own format, at a high quality (the cut shouldn't show):
/// HDR stays HDR where the format can hold it, and 24-bit sound stays 24-bit.
fn codecs(ext: &str, video: bool, audio: bool, info: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut a = Vec::new();
    let hdr = ffmpeg::is_hdr(info);
    let deep = info.get("audioTracks").and_then(|t| t.get(0)).and_then(|t| t["bits"].as_i64()).unwrap_or(16) > 16;
    if video && hdr && matches!(ext, "mp4" | "m4v" | "mov" | "mkv" | "webm") {
        a.extend(ffmpeg::keep_hdr(info, ext == "webm", if ext == "webm" { 30 } else { 20 }));
    } else if video {
        if hdr {
            a.extend(["-vf".to_string(), ffmpeg::to_sdr("yuv420p")]);
            a.extend(ffmpeg::sdr_tags());
        }
        a.extend(match ext {
            "webm" => sv(&["-c:v", "libvpx-vp9", "-crf", "30", "-b:v", "0", "-row-mt", "1"]),
            "avi" => sv(&["-c:v", "mpeg4", "-vtag", "xvid", "-q:v", "3"]),
            "wmv" | "asf" => sv(&["-c:v", "wmv2", "-q:v", "3"]),
            _ => sv(&["-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p"]),
        });
    }
    if audio {
        a.extend(match ext {
            "mp3" | "avi" => sv(&["-c:a", "libmp3lame", "-b:a", "192k"]),
            "wav" => sv(&["-c:a", if deep { "pcm_s24le" } else { "pcm_s16le" }]),
            "aiff" => sv(&["-c:a", if deep { "pcm_s24be" } else { "pcm_s16be" }]),
            "flac" => sv(&["-c:a", "flac", "-sample_fmt", if deep { "s32" } else { "s16" }]),
            "ogg" => sv(&["-c:a", "libvorbis", "-q:a", "6"]),
            "opus" | "webm" => sv(&["-c:a", "libopus", "-b:a", "128k"]),
            "wma" | "wmv" | "asf" => sv(&["-c:a", "wmav2", "-b:a", "192k"]),
            _ => sv(&["-c:a", "aac", "-b:a", "192k"]),
        });
    }
    a
}
