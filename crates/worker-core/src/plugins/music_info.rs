//! Tempo (BPM) and key of songs, shown as soon as they are added; then written into
//! copies as tags (what DJ programs read), and into their names if the name asks for
//! {bpm}, {key} or {camelot}.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{audio, ffmpeg};
use contract::{codes, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};

pub struct MusicInfo;

const RATE: u32 = 22050;
const INPUTS: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "aiff", "aif", "alac", "ape", "wv"];

impl Plugin for MusicInfo {
    fn id(&self) -> &'static str {
        "music-info"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Tempo și tonalitate", "Tempo and key"),
            "summary": t("BPM, tonalitate, Camelot", "BPM, key, Camelot"),
            "keywords": { "ro": ["bpm", "tempo", "tonalitate", "dj", "camelot"], "en": ["bpm finder", "tempo", "key finder", "dj", "camelot"] },
            "category": "audio",
            "icon": "metronome",
            "requires": [ { "engine": "ffmpeg" } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": INPUTS,
                        "nameVariables": [ { "id": "bpm", "label": t("BPM", "BPM") }, { "id": "key", "label": t("tonalitatea", "key") },
                                           { "id": "camelot", "label": t("Camelot", "Camelot") },
                                           { "id": "suffix", "label": t("„ (128 BPM, Am, 8A)”", "“ (128 BPM, Am, 8A)”") } ],
                        "replaceOriginalAllowed": true },
            "analysis": { "fast": ["durationSec", "hasAudio", "bpm", "key", "camelot"] },
            "ui": { "preview": "none", "estimate": false, "showProps": ["bpm", "keyName", "camelot"] },
            "resources": { "class": "light", "diskFactor": 1.1 },
            "presets": [
                { "id": "tags", "label": t("Etichete în copii", "Tags in copies"), "default": true, "values": { "inName": false } },
                { "id": "named", "label": t("Și în nume", "In the name too"), "values": { "inName": true } }
            ],
            "fields": [
                { "key": "inName", "type": "toggle", "default": false, "label": t("Pune BPM-ul și tonalitatea și în numele copiei", "Put the BPM and key in the copy's name too"),
                  "help": t("Etichetele (BPM, tonalitate) se scriu mereu; programele de DJ le citesc.", "The tags (BPM, key) are always written; DJ programs read them.") }
            ],
            "rules": [],
            // Copies of the same kind keep the song's name, in a folder of their own.
            "outputBlock": { "defaults": { "nameTemplate": t("{name}{suffix}", "{name}{suffix}"), "outputDir": "subfolder",
                                           "subfolderName": t("BPM și tonalitate", "BPM and key") } },
            "errors": { "no_audio": t("Fișierul nu are sunet.", "The file has no sound."),
                        "no_beat": t("Nu am găsit un ritm constant.", "No steady beat was found.") }
        })
    }

    /// Up to three minutes from the start are enough for both.
    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let mut p = match ffmpeg::probe(req, f.input_path()) {
            Ok(p) => p,
            Err(e) if e.code == codes::UNSUPPORTED_FILE => return Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) if e.code == codes::CORRUPT_FILE => return Analysis::status("corrupt", codes::CORRUPT_FILE),
            Err(e) => return Analysis::status("failed", &e.code),
        };
        if p.get("hasAudio").and_then(Value::as_bool) == Some(false) {
            return Analysis::ok(p);
        }
        if let Ok(x) = audio::decode(req, f.input_path(), RATE, 1, 180.0) {
            describe(&mut p, &x, req);
        }
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let mut info: Map<String, Value> = f.analysis.clone();
        if !info.contains_key("bpm") && !info.contains_key("key") {
            if info.get("hasAudio").and_then(Value::as_bool) == Some(false) {
                return Err(FileError::new("no_audio"));
            }
            let x = audio::decode(req, input, RATE, 1, 180.0)?;
            describe(&mut info, &x, req);
        }
        let bpm = info.get("bpm").and_then(Value::as_f64);
        let tag = info.get("keyTag").and_then(Value::as_str).unwrap_or("").to_string();
        let camelot = info.get("camelot").and_then(Value::as_str).unwrap_or("").to_string();
        if bpm.is_none() && tag.is_empty() {
            return Err(FileError::new("no_beat"));
        }
        ctx.progress(&f.id, 0.5, "write", 2, 2);
        let ext = input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp3".into());
        let out = ctx.file_dir(&f.id)?.join(format!("result.{ext}"));
        let mut a: Vec<String> = vec!["-i".into(), input.to_string_lossy().into_owned(), "-map".into(), "0".into(), "-c".into(), "copy".into(), "-map_metadata".into(), "0".into()];
        {
            if let Some(b) = bpm {
                a.extend(["-metadata".into(), format!("TBPM={}", b.round() as i64)]);
                a.extend(["-metadata".into(), format!("BPM={}", b.round() as i64)]);
            }
            if !tag.is_empty() {
                // ID3 TKEY ("Am"); other formats read INITIALKEY.
                a.extend(["-metadata".into(), format!("TKEY={tag}"), "-metadata".into(), format!("initialkey={tag}")]);
            }
            if ext == "mp3" {
                a.extend(["-id3v2_version".into(), "3".into()]);
            }
        }
        a.push(out.to_string_lossy().into_owned());
        ffmpeg::run(ctx, &f.id, &a, 0.0, 2, 2)?;
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        let mut vars = json!({ "bpm": bpm.map(|b| (b.round() as i64).to_string()).unwrap_or_default(), "key": tag, "camelot": camelot });
        if req.bool_or("inName", false) {
            // The name the app gives: "{name}" plus " (128 BPM, Am, 8A)".
            let mut bits = Vec::new();
            if let Some(b) = bpm {
                bits.push(format!("{} BPM", b.round() as i64));
            }
            if !tag.is_empty() {
                bits.push(tag.clone());
                bits.push(camelot.clone());
            }
            vars["suffix"] = json!(format!(" ({})", bits.join(", ")));
        }
        ctx.emit_output(&f.id, &out, &ext, vars, in_bytes);
        Ok(Value::Null)
    }
}

/// Tempo and key into the analysis: bpm, keyName (in the interface's words), keyTag, camelot.
fn describe(p: &mut Map<String, Value>, x: &[f32], req: &Request) {
    if let Some(b) = audio::tempo(x, RATE) {
        p.insert("bpm".into(), json!(b));
    }
    if let Some((tonic, minor)) = audio::key(x, RATE) {
        let w = crate::common::words::of(req.locale.as_deref());
        p.insert("key".into(), json!(audio::key_tag(tonic, minor)));
        p.insert("keyTag".into(), json!(audio::key_tag(tonic, minor)));
        p.insert("keyName".into(), json!(audio::key_name(tonic, minor, w.major, w.minor)));
        p.insert("camelot".into(), json!(audio::camelot(tonic, minor)));
    }
}
