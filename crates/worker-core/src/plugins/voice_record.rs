//! Voice recordings: made in the app (the microphone, through the window) or brought as
//! files, then cleaned up for listening: background hiss and hum taken down, the voice
//! made even and clear, silence cut from the start and the end; saved as MP3, M4A or WAV.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct VoiceRecord;

const INPUTS: &[&str] = &["webm", "mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "amr", "3gp", "mp4", "mov", "aiff", "aif", "caf"];

impl Plugin for VoiceRecord {
    fn id(&self) -> &'static str {
        "voice-record"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Înregistrare voce", "Voice recorder"),
            "summary": t("Înregistrează și curăță sunetul", "Record and clean up the sound"),
            "keywords": { "ro": ["înregistrare", "microfon", "reportofon", "zgomot", "voce"], "en": ["record", "microphone", "voice memo", "noise", "voice"] },
            "category": "audio",
            "icon": "microphone",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile", "record": true },
            "output": { "kind": "perInput", "extensions": ["mp3", "m4a", "wav"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasAudio"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.5 },
            "presets": [
                { "id": "clean", "label": t("Voce clară", "Clear voice"), "default": true,
                  "values": { "noise": "medium", "level": true, "trim": true, "format": "mp3" } },
                { "id": "asis", "label": t("Așa cum e", "As it is"), "values": { "noise": "off", "level": false, "trim": false, "format": "mp3" } },
                { "id": "noisy", "label": t("Loc zgomotos", "Noisy place"), "values": { "noise": "strong", "level": true, "trim": true, "format": "mp3" } }
            ],
            "fields": [
                { "key": "noise", "type": "select", "label": t("Reducerea zgomotului", "Noise reduction"), "default": "medium", "display": "chips",
                  "options": [ opt("off", "Fără", "None"), opt("light", "Ușoară", "Light"), opt("medium", "Medie", "Medium"), opt("strong", "Puternică", "Strong") ] },
                { "key": "level", "type": "toggle", "default": true, "label": t("Volum egal și clar", "Even, clear volume"),
                  "help": t("Vocea încet și cea tare ajung la fel de ușor de auzit.", "Quiet and loud speech become equally easy to hear.") },
                { "key": "trim", "type": "toggle", "default": true, "label": t("Taie liniștea de la început și sfârșit", "Cut the silence at the start and end") },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "mp3", "display": "chips",
                  "options": [ opt("mp3", "MP3", "MP3"), opt("m4a", "M4A", "M4A"), opt("wav", "WAV", "WAV") ] }
            ],
            "rules": [
                { "id": "no_audio", "when": { "file": "hasAudio", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au sunet și vor fi sărite.", "Some files have no sound and will be skipped.") }
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

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "encode", 1, 1);
        let info = if f.analysis.contains_key("hasAudio") { f.analysis.clone() } else { ffmpeg::probe(req, input)? };
        if info.get("hasAudio").and_then(Value::as_bool) == Some(false) {
            return Err(FileError::new("no_audio"));
        }
        let duration = info.get("durationSec").and_then(Value::as_f64).unwrap_or(0.0);
        let af = filters(req.str_or("noise", "medium"), req.bool_or("level", true), req.bool_or("trim", true));
        let format = req.str_or("format", "mp3");
        let mut a: Vec<String> = vec!["-i".into(), input.to_string_lossy().into_owned(), "-map".into(), "0:a:0".into(), "-vn".into()];
        if !af.is_empty() {
            a.extend(["-af".into(), af]);
        }
        let codec: &[&str] = match format {
            "wav" => &["-c:a", "pcm_s16le"],
            "m4a" => &["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"],
            _ => &["-c:a", "libmp3lame", "-q:a", "2"],
        };
        a.extend(codec.iter().map(|s| s.to_string()));
        // Speech: one channel is enough and plays the same in both ears.
        a.extend(["-ac".into(), "1".into(), "-ar".into(), "48000".into(), "-map_metadata".into(), "0".into()]);
        let out = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, duration, 1, 1)?;
        ctx.progress(&f.id, 1.0, "encode", 1, 1);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

/// The audio filters, in order: rumble below the voice, the noise floor, the level, the
/// silent ends (trimmed after leveling, so a quiet start is judged right).
fn filters(noise: &str, level: bool, trim: bool) -> String {
    let mut f: Vec<String> = Vec::new();
    if noise != "off" {
        f.push("highpass=f=80".into());
        let nr = match noise {
            "light" => 10,
            "strong" => 30,
            _ => 18,
        };
        // afftdn learns the noise from the quiet parts as it goes.
        f.push(format!("afftdn=nr={nr}:nf=-40:tn=1"));
    }
    if level {
        f.push("dynaudnorm=f=250:g=15:p=0.9:m=8".into());
        f.push("alimiter=limit=0.95".into());
    }
    if trim {
        let cut = "silenceremove=start_periods=1:start_duration=0.25:start_threshold=-45dB:detection=peak";
        f.push(format!("{cut},areverse,{cut},areverse"));
    }
    f.join(",")
}

#[cfg(test)]
mod tests {
    #[test]
    fn filter_chain() {
        assert_eq!(super::filters("off", false, false), "");
        let f = super::filters("medium", true, true);
        assert!(f.starts_with("highpass=f=80,afftdn=nr=18"));
        assert!(f.contains("dynaudnorm") && f.ends_with("areverse"));
    }
}
