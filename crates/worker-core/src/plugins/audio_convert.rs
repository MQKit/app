//! Convert audio between formats, keeping the tags and the album cover, with optional
//! volume, fades, speed and trimming.

use super::video_compress::{head, time_opt};
use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct AudioConvert;

const INPUTS: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "aiff", "aif", "amr", "ac3", "mka", "m4b", "caf", "alac", "ape", "wv"];

impl Plugin for AudioConvert {
    fn id(&self) -> &'static str {
        "audio-convert"
    }

    fn manifest(&self) -> Value {
        let lossy = json!({ "field": "format", "in": ["mp3", "m4a", "ogg", "opus", "wma"] });
        json!({
            "name": t("Conversie audio", "Convert audio"),
            "summary": t("MP3, WAV, FLAC, M4A", "MP3, WAV, FLAC, M4A"),
            "keywords": { "ro": ["audio", "muzică", "mp3", "wav", "flac", "m4a"], "en": ["audio", "music", "mp3", "wav", "flac", "m4a"] },
            "category": "audio",
            "icon": "music",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["mp3", "m4a", "wav", "flac", "ogg", "opus", "aiff", "wma"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasAudio", "audioTracks"] },
            "ui": { "preview": "none", "estimate": true },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [
                { "id": "mp3", "label": t("MP3 pentru orice player", "MP3 for any player"), "default": true, "values": { "format": "mp3", "quality": 192 } },
                { "id": "lossless", "label": t("Fără pierderi (FLAC)", "Lossless (FLAC)"), "values": { "format": "flac" } },
                { "id": "voice", "label": t("Voce, fișier mic", "Voice, small file"), "values": { "format": "opus", "quality": 48, "channels": "mono" } },
                { "id": "apple", "label": t("Pentru iPhone (M4A)", "For iPhone (M4A)"), "values": { "format": "m4a", "quality": 256 } }
            ],
            "groups": [ { "id": "sound", "label": t("Sunet", "Sound"), "order": 1 }, { "id": "cut", "label": t("Doar o parte", "Only a part"), "order": 2 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "mp3", "display": "chips",
                  "options": [ opt("mp3", "MP3", "MP3"), opt("m4a", "M4A", "M4A"), opt("wav", "WAV", "WAV"), opt("flac", "FLAC", "FLAC"),
                               opt("ogg", "OGG", "OGG"), opt("opus", "Opus", "Opus"), opt("aiff", "AIFF", "AIFF"), opt("wma", "WMA", "WMA") ] },
                { "key": "quality", "type": "select", "label": t("Calitate", "Quality"), "default": 192, "showIf": lossy,
                  "options": [ opt(48, "Voce (48 kbps)", "Voice (48 kbps)"), opt(96, "Mică (96 kbps)", "Small (96 kbps)"), opt(128, "Standard (128 kbps)", "Standard (128 kbps)"),
                               opt(192, "Înaltă (192 kbps)", "High (192 kbps)"), opt(256, "Foarte înaltă (256 kbps)", "Very high (256 kbps)"),
                               opt(320, "Maximă (320 kbps)", "Maximum (320 kbps)") ] },
                { "key": "channels", "type": "select", "label": t("Canale", "Channels"), "default": "keep", "display": "chips",
                  "options": [ opt("keep", "Ca în original", "As original"), opt("stereo", "Stereo", "Stereo"), opt("mono", "Mono", "Mono") ] },
                { "key": "sampleRate", "type": "select", "label": t("Frecvență", "Sample rate"), "default": "keep",
                  "options": [ opt("keep", "Ca în original", "As original"), opt(44100, "44,1 kHz (CD)", "44.1 kHz (CD)"), opt(48000, "48 kHz", "48 kHz"),
                               opt(22050, "22 kHz", "22 kHz"), opt(16000, "16 kHz (voce)", "16 kHz (voice)") ] },
                { "key": "normalize", "type": "toggle", "default": false, "label": t("Volum egal (normalizare)", "Even volume (normalize)"),
                  "help": t("Toate piesele ajung la același volum.", "Every track ends up at the same loudness.") },
                { "key": "keepTags", "type": "toggle", "default": true, "label": t("Păstrează titlul, artistul și coperta", "Keep the title, artist and cover") },
                { "key": "volume", "type": "range", "advanced": true, "group": "sound", "min": -20, "max": 20, "step": 1, "unit": " dB", "default": 0,
                  "label": t("Volum", "Volume") },
                { "key": "fadeIn", "type": "range", "advanced": true, "group": "sound", "min": 0, "max": 15, "step": 1, "unit": " s", "default": 0,
                  "label": t("Intrare treptată", "Fade in") },
                { "key": "fadeOut", "type": "range", "advanced": true, "group": "sound", "min": 0, "max": 15, "step": 1, "unit": " s", "default": 0,
                  "label": t("Ieșire treptată", "Fade out") },
                { "key": "tempo", "type": "select", "advanced": true, "group": "sound", "default": 1, "label": t("Viteză", "Speed"),
                  "options": [ opt(0.75, "0,75×", "0.75×"), opt(1, "Normală", "Normal"), opt(1.25, "1,25×", "1.25×"), opt(1.5, "1,5×", "1.5×"), opt(2, "2×", "2×") ] },
                { "key": "start", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("De la", "From"), "placeholder": t("început", "start") },
                { "key": "end", "type": "time", "advanced": true, "group": "cut", "default": "", "label": t("Până la", "To"), "placeholder": t("sfârșit", "end") }
            ],
            "rules": [
                { "id": "no_audio", "when": { "file": "hasAudio", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au sunet și vor fi sărite.", "Some files have no sound and will be skipped.") },
                { "id": "lossy_to_lossless", "severity": "info", "when": { "field": "format", "in": ["flac", "wav", "aiff"] },
                  "message": t("Un MP3 transformat în FLAC sau WAV nu sună mai bine; doar ocupă mai mult.",
                               "An MP3 turned into FLAC or WAV doesn't sound better; it only takes more space.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": { "no_audio": t("Fișierul nu are sunet.", "The file has no sound.") }
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
        let kbps = match req.str_or("format", "mp3") {
            "wav" | "aiff" => 1411.0,
            "flac" => 800.0,
            _ => req.f64_or("quality", 192.0),
        };
        let mut input = 0u64;
        let mut total = 0.0;
        for f in &req.files {
            input += file_size(f.input_path());
            let d = f.analysis.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
            total += d * kbps * 1000.0 / 8.0 / req.f64_or("tempo", 1.0).max(0.5);
        }
        Some(json!({ "inputBytes": input, "outputBytes": total.round(), "confidence": "medium", "method": "formula" }))
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "encode", 1, 1);
        let info = if f.analysis.contains_key("durationSec") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        if info.get("hasAudio").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_audio"));
        }
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let start = time_opt(req, "start").unwrap_or(0.0).max(0.0);
        let end = time_opt(req, "end").filter(|e| *e > 0.0).map(|e| if duration > 0.0 { e.min(duration) } else { e }).unwrap_or(duration);
        if duration > 0.0 && end <= start {
            return Err(FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "durationSec": duration })));
        }
        let format = req.str_or("format", "mp3");
        let kbps = req.f64_or("quality", 192.0) as u32;
        let tempo = req.f64_or("tempo", 1.0).clamp(0.5, 2.0);
        let length = (end - start) / tempo;

        let mut filters: Vec<String> = Vec::new();
        if (tempo - 1.0).abs() > 0.001 {
            filters.push(format!("atempo={tempo}"));
        }
        let vol = req.f64_or("volume", 0.0);
        if vol.abs() > 0.01 {
            filters.push(format!("volume={vol}dB"));
        }
        if req.bool_or("normalize", false) {
            filters.push("loudnorm=I=-16:TP=-1.5:LRA=11".into());
        }
        let fi = req.f64_or("fadeIn", 0.0);
        if fi > 0.0 {
            filters.push(format!("afade=t=in:st=0:d={fi}"));
        }
        let fo = req.f64_or("fadeOut", 0.0);
        if fo > 0.0 && length > fo {
            filters.push(format!("afade=t=out:st={:.3}:d={fo}", length - fo));
        }

        let track = info.get("audioTracks").and_then(|a| a.get(0)).cloned().unwrap_or(Value::Null);
        let src_codec = track["codec"].as_str().unwrap_or("").to_string();
        let src_codec = src_codec.as_str();
        let src_rate = track["sampleRate"].as_i64().unwrap_or(0);
        let src_bits = track["bits"].as_i64().unwrap_or(16);
        let channels = match req.str_or("channels", "keep") {
            "mono" => 1,
            "stereo" => 2,
            _ => track["channels"].as_i64().unwrap_or(2),
        };
        // Opus carries surround only in its standard layouts (5.1, 7.1).
        if format == "opus" && channels > 2 {
            filters.push(if channels > 6 { "aformat=channel_layouts=7.1".into() } else { "aformat=channel_layouts=5.1".into() });
        }
        let same = matches!((format, src_codec), ("mp3", "mp3") | ("flac", "flac") | ("opus", "opus") | ("ogg", "vorbis") | ("m4a", "aac"));
        let untouched = filters.is_empty() && req.str_or("channels", "keep") == "keep" && req.str_or("sampleRate", "keep") == "keep";

        let mut a = head(start, end, duration, input);
        a.extend(sv(&["-map", "0:a:0"]));
        let keep_tags = req.bool_or("keepTags", true);
        // The cover is a picture stream; MP3, M4A and FLAC can carry it.
        let cover = keep_tags && info.get("hasAttachedPic").and_then(Value::as_bool) == Some(true) && matches!(format, "mp3" | "m4a" | "flac");
        if cover {
            a.extend(sv(&["-map", "0:v:0?", "-c:v", "copy", "-disposition:v:0", "attached_pic"]));
        } else {
            a.push("-vn".into());
        }
        if !filters.is_empty() {
            a.extend(["-af".into(), filters.join(",")]);
        }
        match req.str_or("channels", "keep") {
            "mono" => a.extend(sv(&["-ac", "1"])),
            "stereo" => a.extend(sv(&["-ac", "2"])),
            _ => {}
        }
        if let Some(r) = req.opt_f64("sampleRate").filter(|r| *r > 0.0) {
            a.extend(sv(&["-ar", &(r as u32).to_string()]));
        } else if format == "ogg" && src_rate > 48000 {
            // Vorbis at a set bitrate refuses studio rates (96 kHz): 48 kHz sounds the same.
            a.extend(sv(&["-ar", "48000"]));
        }
        // Lossless formats keep a 24-bit source at 24 bits.
        let deep = src_bits > 16;
        if same && untouched {
            // Same format and nothing to change: no second generation of loss.
            a.extend(sv(&["-c:a", "copy"]));
        } else {
            a.extend(match format {
                "mp3" => sv(&["-c:a", "libmp3lame", "-b:a", &format!("{kbps}k")]),
                "m4a" => sv(&["-c:a", "aac", "-b:a", &format!("{kbps}k")]),
                "ogg" => sv(&["-c:a", "libvorbis", "-b:a", &format!("{kbps}k")]),
                "opus" => {
                    let mut o = sv(&["-c:a", "libopus", "-b:a", &format!("{}k", kbps.min(256)), "-application", if kbps <= 64 { "voip" } else { "audio" }]);
                    if channels > 2 {
                        o.extend(sv(&["-mapping_family", "1", "-b:a", &format!("{}k", (kbps.min(256) as i64 * channels / 2).min(512))]));
                    }
                    o
                }
                "wma" => sv(&["-c:a", "wmav2", "-b:a", &format!("{kbps}k")]),
                "flac" if deep => sv(&["-c:a", "flac", "-sample_fmt", "s32"]),
                "flac" => sv(&["-c:a", "flac", "-sample_fmt", "s16"]),
                "aiff" => sv(&["-c:a", if deep { "pcm_s24be" } else { "pcm_s16be" }]),
                _ => sv(&["-c:a", if deep { "pcm_s24le" } else { "pcm_s16le" }]),
            });
        }
        a.extend(sv(&["-map_metadata", if keep_tags { "0" } else { "-1" }]));
        match format {
            "mp3" => a.extend(sv(&["-id3v2_version", "3"])),
            "m4a" => a.extend(sv(&["-movflags", "+faststart"])),
            "aiff" if keep_tags => a.extend(sv(&["-write_id3v2", "1"])),
            _ => {}
        }
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, length.max(0.0), 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(json!({ "copied": same && untouched }))
    }
}

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}
