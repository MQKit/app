//! Subtitles: convert (SRT, WebVTT, ASS), shift the timing, fix broken letters and
//! old encodings, take them out of a video, or put them on one (as a track or burned in).

use super::video_compress::VIDEO_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub struct Subtitles;

const SUBS: &[&str] = &["srt", "vtt", "ass", "ssa"];

impl Plugin for Subtitles {
    fn id(&self) -> &'static str {
        "subtitles"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = SUBS.iter().chain(VIDEO_INPUTS).copied().collect();
        let op = |v: &str| json!({ "field": "operation", "eq": v });
        let mut languages = vec![opt("auto", "Limba Windows", "Your Windows language")];
        languages.extend(LANGUAGES.iter().map(|(_, code, ro, en)| opt(*code, ro, en)));
        json!({
            "name": t("Subtitrări", "Subtitles"),
            "summary": t("Convertește, decalează, repară, lipește", "Convert, shift, fix, attach"),
            "keywords": { "ro": ["subtitrare", "srt", "diacritice", "sincronizare", "decalaj"], "en": ["subtitles", "srt", "captions", "sync", "delay"] },
            "category": "video",
            "icon": "subtitles",
            "requires": [ { "engine": "ffmpeg", "when": { "field": "operation", "in": ["convert", "extract", "attach"] } } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "multiPerInput", "extensions": ["srt", "vtt", "ass", "mkv", "mp4"], "replaceOriginalAllowed": false },
            "analysis": { "fast": [] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.5 },
            "presets": [],
            "groups": [ { "id": "style", "label": t("Aspect", "Look"), "order": 1 } ],
            "fields": [
                { "key": "operation", "type": "select", "label": t("Ce faci", "What to do"), "default": "repair",
                  "options": [ opt("repair", "Repară literele stricate și codarea", "Fix broken letters and encoding"), opt("convert", "Schimbă formatul", "Change the format"),
                               opt("shift", "Decalează (sincronizează)", "Shift (sync)"), opt("extract", "Scoate din video", "Take out of a video"),
                               opt("attach", "Pune pe video", "Put on a video") ] },
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "srt", "display": "chips",
                  "options": [ opt("srt", "SRT", "SRT"), opt("vtt", "WebVTT", "WebVTT"), opt("ass", "ASS", "ASS") ],
                  "showIf": { "field": "operation", "in": ["convert", "extract"] } },
                { "key": "shiftMs", "type": "number", "label": t("Decalaj", "Delay"), "default": 0, "min": -3600000, "max": 3600000, "unit": "ms",
                  "help": t("Pozitiv = subtitrarea apare mai târziu. 1000 ms = o secundă.", "Positive = the subtitles show later. 1000 ms = one second."),
                  "showIf": op("shift") },
                { "key": "fixCedilla", "type": "toggle", "default": true, "label": t("ş ţ (cu sedilă) → ș ț (cu virgulă)", "ş ţ (cedilla) → ș ț (comma)"),
                  "showIf": { "all": [ { "field": "operation", "in": ["repair", "convert", "shift", "attach"] }, { "file": "oldCedilla", "eq": true, "quant": "any" } ] } },
                { "key": "attachMode", "type": "select", "label": t("Cum", "How"), "default": "track", "display": "chips",
                  "options": [ opt("track", "Ca pistă (se poate opri)", "As a track (can be turned off)"), opt("burn", "Arsă în imagine", "Burned into the picture") ],
                  "showIf": op("attach") },
                { "key": "language", "type": "select", "label": t("Limba subtitrării", "Subtitle language"), "default": "auto",
                  "options": languages,
                  "showIf": { "all": [ op("attach"), { "field": "attachMode", "eq": "track" } ] } },
                { "key": "fontSize", "type": "range", "advanced": true, "group": "style", "min": 14, "max": 48, "step": 1, "default": 24,
                  "label": t("Mărimea literelor", "Text size"), "showIf": { "all": [ op("attach"), { "field": "attachMode", "eq": "burn" } ] } },
                { "key": "box", "type": "toggle", "advanced": true, "group": "style", "default": false,
                  "label": t("Fundal întunecat sub text", "Dark box behind the text"), "showIf": { "all": [ op("attach"), { "field": "attachMode", "eq": "burn" } ] } }
            ],
            "rules": [
                // The app gives each file its extension: what doesn't fit the chosen action is said before starting.
                { "id": "needs_subtitles", "severity": "error",
                  "when": { "all": [ { "field": "operation", "in": ["repair", "convert", "shift"] }, { "file": "extension", "notIn": SUBS, "quant": "all" } ] },
                  "message": t("Pentru asta adaugă fișiere de subtitrare (SRT, VTT, ASS). Pentru un video alege „Scoate din video” sau „Pune pe video”.",
                               "For this, add subtitle files (SRT, VTT, ASS). For a video, choose “Take out of a video” or “Put on a video”.") },
                { "id": "videos_skipped", "severity": "warning",
                  "when": { "all": [ { "field": "operation", "in": ["repair", "convert", "shift"] }, { "file": "extension", "notIn": SUBS },
                                     { "not": { "file": "extension", "notIn": SUBS, "quant": "all" } } ] },
                  "message": t("Videourile sunt sărite: acțiunea asta lucrează pe fișiere de subtitrare.", "Videos are skipped: this works on subtitle files.") },
                { "id": "needs_video", "severity": "error",
                  "when": { "all": [ { "field": "operation", "eq": "extract" }, { "file": "extension", "in": SUBS, "quant": "all" } ] },
                  "message": t("Adaugă un video din care să scot subtitrarea.", "Add a video to take the subtitles out of.") },
                { "id": "pairing", "severity": "info", "when": op("attach"),
                  "message": t("Adaugă video-ul și subtitrarea cu același nume (film.mkv și film.srt). Fiecare video primește subtitrarea lui.",
                               "Add the video and the subtitle with the same name (film.mkv and film.srt). Each video gets its own subtitle.") }
            ],
            // {file}: each track taken out of a video is named by its language (film.eng.srt, film.fre.srt).
            "outputBlock": { "defaults": { "nameTemplate": t("{file}", "{file}") } },
            "errors": {
                "not_subtitle": t("Fișierul nu e o subtitrare.", "The file isn't a subtitle."),
                "not_video": t("Fișierul nu e un video.", "The file isn't a video."),
                "no_subtitle_track": t("Video-ul nu are subtitrări text.", "The video has no text subtitles."),
                "no_pair": t("Nu am găsit subtitrarea cu același nume pentru acest video.", "No subtitle with the same name was found for this video."),
                "used_with_video": t("Folosită pentru video-ul cu același nume.", "Used for the video with the same name.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let mut p = Map::new();
        let path = f.input_path();
        if SUBS.contains(&ext_of(path).as_str()) {
            let text = std::fs::read(path).map(|raw| crate::common::text::decode(&raw)).unwrap_or_default();
            p.insert("oldCedilla".into(), json!(old_cedilla(&text)));
        }
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "write", 1, 1);
        let ext = ext_of(path);
        let is_sub = SUBS.contains(&ext.as_str());
        let dir = ctx.file_dir(&f.id)?;
        ctx.progress(&f.id, 0.3, "write", 1, 1);
        match req.str_or("operation", "repair") {
            "extract" => {
                if is_sub {
                    return Err(FileError::new("not_video"));
                }
                let info = ffmpeg::probe(req, path)?;
                let tracks = info.get("subtitleTracks").and_then(Value::as_array).cloned().unwrap_or_default();
                let text: Vec<&Value> = tracks.iter().filter(|s| matches!(s["codec"].as_str(), Some("subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "text"))).collect();
                if text.is_empty() {
                    return Err(FileError::new("no_subtitle_track"));
                }
                let fmt = req.str_or("format", "srt");
                for (n, s) in text.iter().enumerate() {
                    let idx = s["index"].as_i64().unwrap_or(0);
                    let lang = s["language"].as_str().unwrap_or("").to_string();
                    let out = dir.join(format!("track-{}.{fmt}", n + 1));
                    let a = vec!["-i".into(), path.to_string_lossy().into_owned(), "-map".into(), format!("0:s:{idx}"), "-c:s".into(), codec_for(fmt).into(), out.to_string_lossy().into_owned()];
                    ffmpeg::run(ctx, &f.id, &a, 0.0, 1, 1)?;
                    let stem = stem_of(path);
                    let file = if lang.is_empty() || lang == "und" { format!("{stem}.{}", n + 1) } else { format!("{stem}.{lang}") };
                    ctx.emit_output(&f.id, &out, fmt, json!({ "file": file }), if n == 0 { in_bytes } else { 0 });
                }
            }
            "attach" => {
                if is_sub {
                    // The video with the same name uses it.
                    return Err(FileError::skipped("used_with_video", json!({})));
                }
                let sub = pair(req, path).ok_or_else(|| FileError::new("no_pair"))?;
                // A clean UTF-8 copy, so FFmpeg reads every letter right.
                std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
                let clean = ctx.work_dir.join(format!("sub-{}.{}", f.id, ext_of(&sub)));
                let mut text = String::from_utf8(normalize(&std::fs::read(&sub).map_err(super::io_err)?, req.bool_or("fixCedilla", true))).unwrap_or_default();
                if ext_of(&sub) == "srt" {
                    text = tidy_srt(&text);
                }
                std::fs::write(&clean, text).map_err(super::io_err)?;
                let duration = ffmpeg::probe(req, path).ok().and_then(|p| p.get("durationSec").and_then(Value::as_f64)).unwrap_or(0.0);
                let vext = if ext == "mkv" || req.str_or("attachMode", "track") == "burn" { if ext == "mkv" { "mkv" } else { "mp4" } } else { "mp4" };
                let out = dir.join(format!("result.{vext}"));
                let mut a: Vec<String> = vec!["-i".into(), path.to_string_lossy().into_owned()];
                if req.str_or("attachMode", "track") == "burn" {
                    let size = req.f64_or("fontSize", 24.0) as i64;
                    let mut style = format!("FontName=Arial,FontSize={size},PrimaryColour=&H00FFFFFF,OutlineColour=&H00000000,Outline=2,Shadow=0");
                    if req.bool_or("box", false) {
                        style = format!("FontName=Arial,FontSize={size},PrimaryColour=&H00FFFFFF,BackColour=&H80000000,OutlineColour=&H80000000,BorderStyle=3,Outline=4,Shadow=0");
                    }
                    a.extend(["-vf".into(), format!("subtitles='{}':force_style='{style}'", filter_path(&clean))]);
                    a.extend(["-map".into(), "0:v:0".into(), "-map".into(), "0:a?".into(), "-c:v".into(), "libx264".into(), "-crf".into(), "19".into(),
                              "-preset".into(), "medium".into(), "-pix_fmt".into(), "yuv420p".into(), "-c:a".into(), "copy".into()]);
                } else {
                    a.extend(["-i".into(), clean.to_string_lossy().into_owned()]);
                    a.extend(["-map".into(), "0".into(), "-map".into(), "1:0".into(), "-c".into(), "copy".into()]);
                    a.extend(["-c:s".into(), if vext == "mkv" { "srt".into() } else { "mov_text".into() }]);
                    let lang = match req.str_or("language", "auto") {
                        "auto" | "" => track_language(),
                        other => other,
                    };
                    a.extend(["-metadata:s:s:0".into(), format!("language={lang}"), "-disposition:s:0".into(), "default".into()]);
                }
                if vext == "mp4" {
                    a.extend(["-movflags".into(), "+faststart".into()]);
                }
                a.push(out.to_string_lossy().into_owned());
                ffmpeg::run(ctx, &f.id, &a, duration, 1, 1)?;
                ctx.emit_output(&f.id, &out, vext, json!({ "file": stem_of(path) }), in_bytes);
            }
            op => {
                if !is_sub {
                    return Err(FileError::new("not_subtitle"));
                }
                let raw = std::fs::read(path).map_err(super::io_err)?;
                let mut text = String::from_utf8(normalize(&raw, req.bool_or("fixCedilla", true))).unwrap_or_default();
                if ext == "srt" {
                    text = tidy_srt(&text);
                }
                if op == "shift" {
                    text = shift(&text, req.f64_or("shiftMs", 0.0) as i64, &ext);
                }
                let fmt = if op == "convert" { req.str_or("format", "srt").to_string() } else { ext.clone() };
                let out = dir.join(format!("result.{fmt}"));
                if fmt == ext {
                    write_file(&out, &crlf(&text))?;
                } else {
                    std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
                    let tmp = ctx.work_dir.join(format!("in-{}.{ext}", f.id));
                    std::fs::write(&tmp, text.as_bytes()).map_err(super::io_err)?;
                    let a = vec!["-i".into(), tmp.to_string_lossy().into_owned(), "-c:s".into(), codec_for(&fmt).into(), out.to_string_lossy().into_owned()];
                    ffmpeg::run(ctx, &f.id, &a, 0.0, 1, 1)?;
                }
                ctx.emit_output(&f.id, &out, &fmt, json!({ "file": stem_of(path) }), in_bytes);
            }
        }
        ctx.progress(&f.id, 1.0, "write", 1, 1);
        Ok(Value::Null)
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn codec_for(fmt: &str) -> &'static str {
    match fmt {
        "vtt" => "webvtt",
        "ass" | "ssa" => "ass",
        _ => "srt",
    }
}

/// The subtitle file among the inputs that goes with this video: same name, or
/// `film.ro.srt` for `film.mkv`; the only subtitle if there is just one.
fn pair(req: &Request, video: &Path) -> Option<PathBuf> {
    let stem = stem_of(video).to_lowercase();
    let subs: Vec<&PathBuf> = req.files.iter().map(|f| f.input_path()).filter(|p| SUBS.contains(&ext_of(p).as_str())).collect();
    subs.iter()
        .find(|p| {
            let s = stem_of(p).to_lowercase();
            s == stem || s.starts_with(&format!("{stem}."))
        })
        .or(if subs.len() == 1 { subs.first() } else { None })
        .map(|p| p.to_path_buf())
}

/// A path for FFmpeg's subtitles filter: forward slashes, the drive colon escaped.
fn filter_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}

/// Bytes in any encoding → UTF-8 text, with letters broken by a wrong encoding repaired
/// and, if asked, the old cedilla forms ş ţ turned into ș ț where the text needs it.
fn normalize(raw: &[u8], fix_cedilla: bool) -> Vec<u8> {
    let mut s = crate::common::text::decode(raw);
    // UTF-8 that was saved again as Windows-1252 somewhere ("Ã©" for "é", "È›" for "ț"),
    // repaired line by line: a line is replaced only when undoing it gives valid, shorter UTF-8.
    if !s.is_ascii() {
        s = s.split_inclusive('\n').map(|line| unmojibake(line).unwrap_or_else(|| line.to_string())).collect();
    }
    if fix_cedilla && old_cedilla(&s) {
        s = s.replace('ş', "ș").replace('ţ', "ț").replace('Ş', "Ș").replace('Ţ', "Ț");
    }
    s.into_bytes()
}

/// One line of double-encoded text back to what it was, or None if it wasn't broken.
/// The five bytes Windows-1252 leaves undefined (0x81 0x8D 0x8F 0x90 0x9D) often reach us
/// as U+FFFD: each gets the one of them that leaves the line valid (” ends in 0x9D).
fn unmojibake(line: &str) -> Option<String> {
    if line.is_ascii() {
        return None;
    }
    let mut bytes = Vec::with_capacity(line.len());
    let mut holes = Vec::new();
    for ch in line.chars() {
        if ch == '\u{FFFD}' {
            holes.push(bytes.len());
            bytes.push(0x9D);
            continue;
        }
        let mut buf = [0u8; 4];
        let (b, _, bad) = encoding_rs::WINDOWS_1252.encode(ch.encode_utf8(&mut buf));
        if bad {
            return None; // a character no Windows-1252 text could have produced
        }
        bytes.extend_from_slice(&b);
    }
    let errors = |b: &[u8]| String::from_utf8_lossy(b).matches('\u{FFFD}').count();
    for i in holes {
        let best = [0x9Du8, 0x81, 0x8D, 0x8F, 0x90]
            .into_iter()
            .min_by_key(|c| {
                let mut t = bytes.clone();
                t[i] = *c;
                errors(&t)
            })
            .unwrap_or(0x9D);
        bytes[i] = best;
    }
    let fixed = String::from_utf8(bytes).ok()?;
    (fixed.chars().count() < line.chars().count()).then_some(fixed)
}

/// Romanian text typed with the old cedilla letters: ţ (no other language uses it), or ş
/// next to ă. Turkish, where ş is right, never has either, so it is left alone.
fn old_cedilla(s: &str) -> bool {
    s.contains(['ţ', 'Ţ']) || (s.contains(['ş', 'Ş']) && s.contains(['ă', 'Ă']))
}

/// The ISO 639-2 code for a subtitle track in the user's Windows language.
fn track_language() -> &'static str {
    let lang = crate::common::winrt::user_language();
    LANGUAGES.iter().find(|(two, _, _, _)| *two == lang).map(|l| l.1).unwrap_or("und")
}

/// (Windows primary tag, ISO 639-2 code, Romanian name, English name), by English name.
const LANGUAGES: &[(&str, &str, &str, &str)] = &[
    ("ar", "ara", "Arabă", "Arabic"), ("bg", "bul", "Bulgară", "Bulgarian"), ("ca", "cat", "Catalană", "Catalan"),
    ("zh", "chi", "Chineză", "Chinese"), ("hr", "hrv", "Croată", "Croatian"), ("cs", "cze", "Cehă", "Czech"),
    ("da", "dan", "Daneză", "Danish"), ("nl", "dut", "Olandeză", "Dutch"), ("en", "eng", "Engleză", "English"),
    ("et", "est", "Estonă", "Estonian"), ("fi", "fin", "Finlandeză", "Finnish"), ("fr", "fre", "Franceză", "French"),
    ("de", "ger", "Germană", "German"), ("el", "gre", "Greacă", "Greek"), ("he", "heb", "Ebraică", "Hebrew"),
    ("hi", "hin", "Hindi", "Hindi"), ("hu", "hun", "Maghiară", "Hungarian"), ("id", "ind", "Indoneziană", "Indonesian"),
    ("it", "ita", "Italiană", "Italian"), ("ja", "jpn", "Japoneză", "Japanese"), ("ko", "kor", "Coreeană", "Korean"),
    ("lv", "lav", "Letonă", "Latvian"), ("lt", "lit", "Lituaniană", "Lithuanian"), ("ms", "may", "Malaeză", "Malay"),
    ("nb", "nor", "Norvegiană", "Norwegian"), ("fa", "per", "Persană", "Persian"), ("pl", "pol", "Poloneză", "Polish"),
    ("pt", "por", "Portugheză", "Portuguese"), ("ro", "rum", "Română", "Romanian"), ("ru", "rus", "Rusă", "Russian"),
    ("sr", "srp", "Sârbă", "Serbian"), ("sk", "slo", "Slovacă", "Slovak"), ("sl", "slv", "Slovenă", "Slovenian"),
    ("es", "spa", "Spaniolă", "Spanish"), ("sv", "swe", "Suedeză", "Swedish"), ("th", "tha", "Thailandeză", "Thai"),
    ("tr", "tur", "Turcă", "Turkish"), ("uk", "ukr", "Ucraineană", "Ukrainian"), ("vi", "vie", "Vietnameză", "Vietnamese"),
];

fn crlf(s: &str) -> Vec<u8> {
    s.replace("\r\n", "\n").replace('\n', "\r\n").into_bytes()
}

/// Moves every time in an SRT, WebVTT or ASS text by `ms`; cues that end before 0 go.
fn shift(text: &str, ms: i64, ext: &str) -> String {
    if ms == 0 {
        return text.to_string();
    }
    let ass = matches!(ext, "ass" | "ssa");
    let mut out = String::with_capacity(text.len());
    let mut drop_block = false;
    for line in text.split_inclusive('\n') {
        if ass {
            if let Some(rest) = line.strip_prefix("Dialogue:") {
                let mut parts: Vec<String> = rest.splitn(10, ',').map(String::from).collect();
                if parts.len() >= 3 {
                    let (a, b) = (parse_ts(parts[1].trim()), parse_ts(parts[2].trim()));
                    if let (Some(a), Some(b)) = (a, b) {
                        if b + ms <= 0 {
                            continue;
                        }
                        parts[1] = fmt_ass(a + ms);
                        parts[2] = fmt_ass(b + ms);
                        out.push_str("Dialogue:");
                        out.push_str(&parts.join(","));
                        continue;
                    }
                }
            }
            out.push_str(line);
            continue;
        }
        if let Some((l, r)) = line.split_once("-->") {
            let r_time = r.split_whitespace().next().unwrap_or("");
            if let (Some(a), Some(b)) = (parse_ts(l.trim()), parse_ts(r_time)) {
                if b + ms <= 0 {
                    drop_block = true;
                    // Remove the cue number written just before.
                    let trimmed = out.trim_end_matches(['\r', '\n']).to_string();
                    if let Some(pos) = trimmed.rfind('\n') {
                        if trimmed[pos + 1..].trim().chars().all(|c| c.is_ascii_digit()) {
                            out.truncate(pos + 1);
                        }
                    } else if trimmed.trim().chars().all(|c| c.is_ascii_digit()) {
                        out.clear();
                    }
                    continue;
                }
                let vtt = ext == "vtt";
                let tail = r.trim_start().strip_prefix(r_time).unwrap_or("");
                out.push_str(&format!("{} --> {}{}", fmt_ts(a + ms, vtt), fmt_ts(b + ms, vtt), tail));
                continue;
            }
        }
        if drop_block {
            if line.trim().is_empty() {
                drop_block = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

/// "01:02:03,456", "01:02:03.456", "02:03.456" or ASS "1:02:03.45" → milliseconds.
/// An SRT as players expect it: numbered from 1, times as `00:00:01,500` (files often
/// have dots, short milliseconds or no numbers, and FFmpeg refuses them).
fn tidy_srt(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = String::new();
    let mut n = 0;
    for block in text.split("\n\n").map(|b| b.trim_matches('\n')).filter(|b| !b.trim().is_empty()) {
        let lines: Vec<&str> = block.lines().collect();
        let Some(t) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let mut times = lines[t].splitn(2, "-->");
        let (Some(a), Some(b)) = (times.next(), times.next()) else { continue };
        // What follows the end time (VTT-style positions) is dropped: SRT has none.
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (parse_ts(a.trim()), parse_ts(b)) else { continue };
        n += 1;
        out.push_str(&format!("{n}\n{} --> {}\n", fmt_ts(start, false), fmt_ts(end, false)));
        for l in &lines[t + 1..] {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
    }
    if n == 0 { text } else { out }
}

fn parse_ts(s: &str) -> Option<i64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?, *s),
        [m, s] => (0, m.parse::<i64>().ok()?, *s),
        _ => return None,
    };
    let secs: f64 = sec.parse().ok()?;
    Some(((h * 3600 + m * 60) as f64 * 1000.0 + secs * 1000.0).round() as i64)
}

fn fmt_ts(ms: i64, vtt: bool) -> String {
    let ms = ms.max(0);
    let (h, m, s, r) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h:02}:{m:02}:{s:02}{}{r:03}", if vtt { '.' } else { ',' })
}

fn fmt_ass(ms: i64) -> String {
    let ms = ms.max(0);
    let (h, m, s, cs) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000 / 10);
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

#[cfg(test)]
mod tests {
    use super::normalize;

    fn fix(s: &str) -> String {
        String::from_utf8(normalize(s.as_bytes(), true)).unwrap()
    }

    #[test]
    fn double_encoded_text_is_repaired_and_good_text_left_alone() {
        // UTF-8 read as Windows-1252, the 0x9D of ” lost on the way.
        let broken = "ÈšarÄƒ, È™oseaua È™i Ã®nÈ›elegerea: ÄƒÃ¢Ã®\nAlt rÃ¢nd â€” â€žghilimeleâ€\u{FFFD}\n";
        assert_eq!(fix(broken), "Țară, șoseaua și înțelegerea: ăâî\nAlt rând — „ghilimele”\n");
        assert_eq!(fix("Ð¿Ñ€Ð¸Ð²ÐµÑ‚\n"), "привет\n");
        assert_eq!(fix("cafÃ© crÃ¨me\n"), "café crème\n");
        // Correct text, Romanian included, stays exactly as it is.
        for good in ["Țară și înțelegere: ăâî — „da”\n", "Âmbătrânire în câmp\n", "café\n", "Größe\n", "привет\n"] {
            assert_eq!(fix(good), good);
        }
        // Only the broken lines of a mixed file change.
        assert_eq!(fix("bun: ăâî\nstricat: È›\n"), "bun: ăâî\nstricat: ț\n");
    }

    #[test]
    fn srt_tidied() {
        let messy = "00:00:01.5 --> 00:00:02.500 X:10\nUnu\n\n\n7\n0:00:03,000 --> 0:00:04,000\nDoi\ndoi\n";
        assert_eq!(super::tidy_srt(messy), "1\n00:00:01,500 --> 00:00:02,500\nUnu\n\n2\n00:00:03,000 --> 00:00:04,000\nDoi\ndoi\n\n");
    }
}
