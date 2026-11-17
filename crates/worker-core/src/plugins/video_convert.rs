//! Convert video between containers (MP4, MKV, WebM, MOV, AVI). Tracks the new
//! container accepts are copied as they are; only the rest is re-encoded.

use super::video_compress::{cap_resolution, head, resolution_options, trim, Source, VIDEO_INPUTS};
use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct VideoConvert;

impl Plugin for VideoConvert {
    fn id(&self) -> &'static str {
        "video-convert"
    }

    fn manifest(&self) -> Value {
        let not_remux = json!({ "field": "mode", "ne": "remux" });
        json!({
            "name": t("Conversie video", "Convert video"),
            "summary": t("MP4, MKV, WebM, MOV", "MP4, MKV, WebM, MOV"),
            "keywords": { "ro": ["mkv în mp4", "mov în mp4", "avi", "webm", "format video", "schimbă formatul"], "en": ["mkv to mp4", "mov to mp4", "avi", "webm", "video format", "change format"] },
            "category": "video",
            "icon": "movie",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": VIDEO_INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["mp4", "mkv", "webm", "mov", "avi"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasVideo", "hasAudio", "width", "height"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 1.5 },
            "presets": [
                { "id": "compat", "label": t("MP4 care merge peste tot", "MP4 that plays everywhere"), "default": true, "values": { "format": "mp4", "mode": "auto" } },
                { "id": "remux", "label": t("Doar schimbă formatul (instant)", "Just change the format (instant)"), "values": { "mode": "remux" } },
                { "id": "web", "label": t("Pentru web (WebM)", "For the web (WebM)"), "values": { "format": "webm", "mode": "auto" } },
                { "id": "old", "label": t("Player vechi (AVI)", "Old player (AVI)"), "values": { "format": "avi", "mode": "reencode", "maxResolution": 480 } }
            ],
            "groups": [ { "id": "picture", "label": t("Imagine", "Picture"), "order": 1 }, { "id": "cut", "label": t("Doar o parte", "Only a part"), "order": 2 },
                        { "id": "more", "label": t("Detalii", "Details"), "order": 3 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "mp4", "display": "chips",
                  "options": [ opt("mp4", "MP4", "MP4"), opt("mkv", "MKV", "MKV"), opt("webm", "WebM", "WebM"), opt("mov", "MOV", "MOV"), opt("avi", "AVI", "AVI") ] },
                { "key": "mode", "type": "select", "label": t("Cum", "How"), "default": "auto",
                  "options": [ opt("auto", "Automat (copiază ce se poate)", "Automatic (copies what it can)"),
                               opt("remux", "Doar formatul, fără recodare", "Format only, no re-encoding"),
                               opt("reencode", "Recodează tot", "Re-encode everything") ] },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 10, "step": 1, "default": 7, "showIf": not_remux },
                { "key": "maxResolution", "type": "select", "label": t("Rezoluție maximă", "Maximum resolution"), "default": "keep",
                  "options": resolution_options(), "showIf": not_remux },
                { "key": "subtitles", "type": "select", "label": t("Subtitrări", "Subtitles"), "default": "keep", "display": "chips",
                  "options": [ opt("keep", "Păstrează", "Keep"), opt("remove", "Fără", "None") ] },
                { "key": "rotate", "type": "select", "advanced": true, "group": "picture", "default": 0, "label": t("Rotire", "Rotate"),
                  "options": [ opt(0, "Fără", "None"), opt(90, "90° la dreapta", "90° right"), opt(180, "180°", "180°"), opt(270, "90° la stânga", "90° left") ],
                  "showIf": not_remux },
                { "key": "flip", "type": "select", "advanced": true, "group": "picture", "default": "none", "label": t("Oglindire", "Mirror"),
                  "options": [ opt("none", "Fără", "None"), opt("h", "Stânga-dreapta", "Left-right"), opt("v", "Sus-jos", "Up-down") ], "showIf": not_remux },
                { "key": "fps", "type": "select", "advanced": true, "group": "picture", "default": "keep", "label": t("Cadre pe secundă", "Frames per second"),
                  "options": [ opt("keep", "Păstrează", "Keep"), opt(60, "60", "60"), opt(50, "50", "50"), opt(30, "30", "30"), opt(25, "25", "25"), opt(24, "24", "24") ],
                  "showIf": not_remux },
                { "key": "start", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("De la", "From"), "placeholder": t("început", "start") },
                { "key": "end", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("Până la", "To"), "placeholder": t("sfârșit", "end") },
                { "key": "keepMetadata", "type": "toggle", "advanced": true, "group": "more", "default": true, "label": t("Păstrează data și locația", "Keep the date and location") }
            ],
            "rules": [
                { "id": "no_video", "when": { "file": "hasVideo", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au imagine și vor fi sărite.", "Some files have no picture and will be skipped.") },
                { "id": "remux_cut", "severity": "info", "when": { "all": [ { "field": "mode", "eq": "remux" }, { "field": "start", "ne": "" } ] },
                  "message": t("Fără recodare, tăietura începe la cel mai apropiat cadru-cheie, deci poate fi cu o secundă mai devreme.",
                               "Without re-encoding, the cut starts at the nearest keyframe, so it may be a second early.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "no_video": t("Fișierul nu are imagine.", "The file has no picture."),
                "remux_incompatible": t("Formatul nou nu acceptă pistele acestui fișier fără recodare. Alege „Automat”.",
                                        "The new format can't hold this file's tracks without re-encoding. Choose \u{201C}Automatic\u{201D}.")
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
        let info = if f.analysis.contains_key("subtitleTracks") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        if info.get("hasVideo").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_video"));
        }
        let format = req.str_or("format", "mp4");
        let mode = req.str_or("mode", "auto");
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let (start, end) = trim(req, duration);
        let source = Source::of(&info);
        let vcodec = info.get("video").and_then(|v| v["codec"].as_str()).unwrap_or("").to_string();
        let vcodec = vcodec.as_str();
        let pix = info.get("video").and_then(|v| v["pixFmt"].as_str()).unwrap_or("").to_string();

        // Picture changes force a re-encode of the video.
        let short = cap_resolution(req, &source);
        let fps = req.opt_f64("fps").filter(|f| *f > 0.0);
        let rotate = req.f64_or("rotate", 0.0) as i64;
        let flip = req.str_or("flip", "none");
        let changes = mode != "remux" && (short < source.short() - 0.5 || fps.is_some() || rotate != 0 || flip != "none");
        let copy_video = mode == "remux" || (mode == "auto" && !changes && video_fits(format, vcodec) && plays_everywhere(format, vcodec, &pix));
        if mode == "remux" && !video_fits(format, vcodec) {
            return Err(FileError::with("remux_incompatible", json!({ "codec": vcodec, "format": format })));
        }

        let mut a = head(start, end, duration, input);
        a.extend(sv(&["-map", "0:v:0"]));
        if copy_video {
            a.extend(sv(&["-c:v", "copy"]));
        } else {
            let mut vf = Vec::new();
            match rotate {
                90 => vf.push("transpose=1".to_string()),
                180 => vf.push("transpose=1,transpose=1".to_string()),
                270 => vf.push("transpose=2".to_string()),
                _ => {}
            }
            match flip {
                "h" => vf.push("hflip".into()),
                "v" => vf.push("vflip".into()),
                _ => {}
            }
            if short < source.short() - 0.5 {
                let s = (short as i64 / 2) * 2;
                // After rotating, the short side may be the width: let the filter decide.
                vf.push(format!("scale=w='if(gte(iw,ih),-2,{s})':h='if(gte(iw,ih),{s},-2)'"));
            }
            if let Some(r) = fps {
                vf.push(format!("fps={r}"));
            }
            let hdr = ffmpeg::is_hdr(&info);
            vf.push(if hdr { ffmpeg::to_sdr("yuv420p") } else { "format=yuv420p".into() });
            a.extend(["-vf".into(), vf.join(",")]);
            if hdr {
                a.extend(ffmpeg::sdr_tags());
            }
            let q = req.f64_or("quality", 7.0).clamp(1.0, 10.0);
            match format {
                "webm" => a.extend(sv(&["-c:v", "libvpx-vp9", "-crf", &((45.0 - 2.5 * q).round() as i64).to_string(), "-b:v", "0", "-row-mt", "1", "-deadline", "good", "-cpu-used", "2"])),
                "avi" => a.extend(sv(&["-c:v", "mpeg4", "-vtag", "xvid", "-q:v", &((13.0 - q).clamp(2.0, 12.0) as i64).to_string()])),
                _ => a.extend(sv(&["-c:v", "libx264", "-preset", "medium", "-crf", &((35.0 - 2.0 * q).round() as i64).to_string()])),
            }
        }

        // Every audio track: copied when the container takes it, else encoded.
        let tracks = info.get("audioTracks").and_then(Value::as_array).cloned().unwrap_or_default();
        for (i, tr) in tracks.iter().enumerate() {
            let codec = tr["codec"].as_str().unwrap_or("");
            a.extend(sv(&["-map", &format!("0:a:{i}")]));
            if mode == "remux" && audio_fits(format, codec) || mode == "auto" && audio_fits(format, codec) && audio_plays_everywhere(format, codec) {
                a.extend(sv(&[&format!("-c:a:{i}"), "copy"]));
            } else if mode == "remux" {
                return Err(FileError::with("remux_incompatible", json!({ "codec": codec, "format": format })));
            } else {
                let (c, b) = match format {
                    "webm" => ("libopus", "128k"),
                    "avi" => ("libmp3lame", "192k"),
                    _ => ("aac", "160k"),
                };
                a.extend(sv(&[&format!("-c:a:{i}"), c, &format!("-b:a:{i}"), b]));
            }
        }

        // Subtitles: text ones are converted to what the container uses; pictures only fit MKV.
        if req.str_or("subtitles", "keep") == "keep" {
            let subs = info.get("subtitleTracks").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut n = 0;
            for s in &subs {
                let codec = s["codec"].as_str().unwrap_or("");
                let text = matches!(codec, "subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "text");
                let target = match format {
                    "mkv" => Some(if text && codec == "mov_text" { "srt" } else { "copy" }),
                    "mp4" | "mov" if text => Some("mov_text"),
                    "webm" if text => Some("webvtt"),
                    _ => None,
                };
                if let Some(tg) = target {
                    let idx = s["index"].as_i64().unwrap_or(0);
                    a.extend(sv(&["-map", &format!("0:s:{idx}"), &format!("-c:s:{n}"), tg]));
                    n += 1;
                }
            }
        }
        a.extend(sv(&["-dn"]));
        if req.bool_or("keepMetadata", true) {
            a.extend(sv(&["-map_metadata", "0", "-map_chapters", "0"]));
        } else {
            a.extend(sv(&["-map_metadata", "-1", "-map_chapters", "-1"]));
        }
        if matches!(format, "mp4" | "mov") {
            a.extend(sv(&["-movflags", "+faststart"]));
        }
        // Apple players (and Windows') need HEVC tagged hvc1 in MP4 and MOV.
        if matches!(format, "mp4" | "mov") && copy_video && vcodec == "hevc" {
            a.extend(sv(&["-tag:v", "hvc1"]));
        }
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, (end - start).max(0.0), 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(json!({ "copiedVideo": copy_video }))
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Can the container hold this video codec as it is?
fn video_fits(format: &str, codec: &str) -> bool {
    match format {
        "mkv" => true,
        "mp4" => matches!(codec, "h264" | "hevc" | "av1" | "mpeg4" | "vp9"),
        "mov" => matches!(codec, "h264" | "hevc" | "mpeg4" | "prores" | "mjpeg"),
        "webm" => matches!(codec, "vp8" | "vp9" | "av1"),
        // Old AVI players expect MPEG-4 (Xvid); H.264 in AVI would also need a bitstream filter.
        "avi" => matches!(codec, "mpeg4" | "msmpeg4v3" | "mjpeg"),
        _ => false,
    }
}

/// In "automatic" mode a stream is copied only if phones, Windows, Macs and browsers all
/// play it in that container: VP9 or AV1 in MP4 and H.264 in 4:4:4 are valid but play in
/// few places, so they are re-encoded (the "copy" mode still copies them).
fn plays_everywhere(format: &str, codec: &str, pix: &str) -> bool {
    match format {
        "mp4" | "mov" => match codec {
            "h264" => matches!(pix, "yuv420p" | "yuvj420p" | ""),
            "hevc" => matches!(pix, "yuv420p" | "yuvj420p" | "yuv420p10le" | ""),
            "prores" | "mjpeg" => format == "mov",
            "mpeg4" => true,
            _ => false,
        },
        _ => true,
    }
}

fn audio_plays_everywhere(format: &str, codec: &str) -> bool {
    match format {
        "mp4" | "mov" => matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "alac" | "pcm_s16le" | "pcm_s24le"),
        _ => true,
    }
}

/// Can the container hold this audio codec as it is?
fn audio_fits(format: &str, codec: &str) -> bool {
    match format {
        "mkv" => true,
        "mp4" => matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "alac" | "opus" | "flac"),
        "mov" => matches!(codec, "aac" | "mp3" | "ac3" | "alac" | "pcm_s16le" | "pcm_s24le"),
        "webm" => matches!(codec, "opus" | "vorbis"),
        "avi" => matches!(codec, "mp3" | "ac3" | "pcm_s16le"),
        _ => false,
    }
}
