//! Transcribe: speech in audio and video to text or subtitles, with Whisper (MIT,
//! whisper.cpp compiled into the worker; the model is a download). FFmpeg turns any
//! input into 16 kHz mono; everything stays on the PC.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::ffmpeg;
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Value};
use std::io::Read;
use std::process::{Command, Stdio};

pub const MEDIA: &[&str] = &["mp3", "wav", "m4a", "aac", "ogg", "opus", "flac", "wma", "amr", "mp4", "mkv", "mov", "avi", "webm", "wmv", "m4v", "3gp", "mpg", "mpeg", "ts"];

struct Model {
    engine: &'static str,
    file: &'static str,
}

const FAST: Model = Model { engine: "whisper-small", file: "ggml-small-q5_1.bin" };
const ACCURATE: Model = Model { engine: "whisper-turbo", file: "ggml-large-v3-turbo-q5_0.bin" };

fn model(req: &Request) -> &'static Model {
    if req.str_or("model", "accurate") == "fast" { &FAST } else { &ACCURATE }
}

/// Whisper's languages, each named in its own language (so none comes first by default).
const LANGUAGES: &[(&str, &str)] = &[
    ("af", "Afrikaans"), ("am", "አማርኛ"), ("ar", "العربية"), ("as", "অসমীয়া"), ("az", "Azərbaycan"), ("ba", "Башҡорт"), ("be", "Беларуская"),
    ("bg", "Български"), ("bn", "বাংলা"), ("bo", "བོད་སྐད་"), ("br", "Brezhoneg"), ("bs", "Bosanski"), ("ca", "Català"), ("cs", "Čeština"),
    ("cy", "Cymraeg"), ("da", "Dansk"), ("de", "Deutsch"), ("el", "Ελληνικά"), ("en", "English"), ("es", "Español"), ("et", "Eesti"),
    ("eu", "Euskara"), ("fa", "فارسی"), ("fi", "Suomi"), ("fo", "Føroyskt"), ("fr", "Français"), ("gl", "Galego"), ("gu", "ગુજરાતી"),
    ("ha", "Hausa"), ("haw", "ʻŌlelo Hawaiʻi"), ("he", "עברית"), ("hi", "हिन्दी"), ("hr", "Hrvatski"), ("ht", "Kreyòl ayisyen"), ("hu", "Magyar"),
    ("hy", "Հայերեն"), ("id", "Bahasa Indonesia"), ("is", "Íslenska"), ("it", "Italiano"), ("ja", "日本語"), ("jw", "Basa Jawa"), ("ka", "ქართული"),
    ("kk", "Қазақ"), ("km", "ខ្មែរ"), ("kn", "ಕನ್ನಡ"), ("ko", "한국어"), ("la", "Latina"), ("lb", "Lëtzebuergesch"), ("ln", "Lingála"),
    ("lo", "ລາວ"), ("lt", "Lietuvių"), ("lv", "Latviešu"), ("mg", "Malagasy"), ("mi", "Māori"), ("mk", "Македонски"), ("ml", "മലയാളം"),
    ("mn", "Монгол"), ("mr", "मराठी"), ("ms", "Bahasa Melayu"), ("mt", "Malti"), ("my", "မြန်မာ"), ("ne", "नेपाली"), ("nl", "Nederlands"),
    ("nn", "Nynorsk"), ("no", "Norsk"), ("oc", "Occitan"), ("pa", "ਪੰਜਾਬੀ"), ("pl", "Polski"), ("ps", "پښتو"), ("pt", "Português"),
    ("ro", "Română"), ("ru", "Русский"), ("sa", "संस्कृतम्"), ("sd", "سنڌي"), ("si", "සිංහල"), ("sk", "Slovenčina"), ("sl", "Slovenščina"),
    ("sn", "chiShona"), ("so", "Soomaali"), ("sq", "Shqip"), ("sr", "Српски"), ("su", "Basa Sunda"), ("sv", "Svenska"), ("sw", "Kiswahili"),
    ("ta", "தமிழ்"), ("te", "తెలుగు"), ("tg", "Тоҷикӣ"), ("th", "ไทย"), ("tk", "Türkmen"), ("tl", "Tagalog"), ("tr", "Türkçe"),
    ("tt", "Татар"), ("uk", "Українська"), ("ur", "اردو"), ("uz", "Oʻzbek"), ("vi", "Tiếng Việt"), ("yi", "ייִדיש"), ("yo", "Yorùbá"),
    ("yue", "粵語"), ("zh", "中文"),
];

pub struct Transcribe;

impl Plugin for Transcribe {
    fn id(&self) -> &'static str {
        "transcribe"
    }

    fn manifest(&self) -> Value {
        let mut languages = vec![opt("auto", "Recunoaște singur", "Detect it")];
        languages.extend(LANGUAGES.iter().map(|(code, name)| json!({ "value": code, "label": t(name, name) })));
        json!({
            "name": t("Transcriere", "Transcribe"),
            "summary": t("Din vorbire în text și subtitrări", "Speech to text and subtitles"),
            "keywords": { "ro": ["transcriere", "din audio în text", "subtitrări automate", "dictare", "whisper"],
                          "en": ["transcribe", "speech to text", "audio to text", "automatic subtitles", "whisper"] },
            "category": "audio",
            "icon": "microphone",
            "requires": [ { "engine": "ffmpeg" },
                          { "engine": FAST.engine, "when": { "field": "model", "eq": "fast" } },
                          { "engine": ACCURATE.engine, "when": { "field": "model", "ne": "fast" } } ],
            "input": { "extensions": MEDIA, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["txt", "docx", "srt", "vtt"], "replaceOriginalAllowed": false },
            "analysis": { "fast": [] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 0.1 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Rezultat", "Result"), "default": "txt", "display": "chips",
                  "options": [ opt("txt", "Text", "Text"), opt("docx", "Word", "Word"), opt("srt", "Subtitrare SRT", "SRT subtitles"), opt("vtt", "Subtitrare VTT", "VTT subtitles") ] },
                { "key": "language", "type": "select", "label": t("Limba vorbită", "Spoken language"), "default": "auto", "options": languages },
                { "key": "model", "type": "select", "label": t("Model", "Model"), "default": "accurate",
                  "options": [ opt("accurate", "Precis (recomandat)", "Accurate (recommended)"), opt("fast", "Rapid, mai puțin precis", "Fast, less accurate") ] },
                { "key": "translate", "type": "toggle", "default": false, "advanced": true, "group": "more",
                  "label": t("Tradu în engleză", "Translate to English"),
                  "help": t("Modelul Whisper poate traduce doar în engleză.", "The Whisper model can only translate into English.") },
                { "key": "prompt", "type": "text", "default": "", "advanced": true, "group": "more",
                  "label": t("Cuvinte de recunoscut (opțional)", "Words to recognize (optional)"),
                  "placeholder": t("Nume, termeni: Ionescu, ONNX, Cluj", "Names, terms: Ionescu, ONNX, Cluj"),
                  "help": t("Ajută modelul să scrie corect nume și termeni rari.", "Helps the model spell names and rare terms right.") }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "no_speech": t("Nu am auzit vorbire în fișier.", "No speech was heard in the file."),
                "no_audio": t("Fișierul nu are sunet.", "The file has no sound."),
                "old_processor": t("Procesorul acestui PC e prea vechi pentru transcriere (are nevoie de AVX2, din 2013 încoace).",
                                   "This PC's processor is too old for transcription (it needs AVX2, from 2013 on).")
            }
        })
    }

    fn analyze(&self, _f: &FileRef, _req: &Request) -> Analysis {
        Analysis::ok(Default::default())
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        // whisper.cpp is built for AVX2 (crates/whisper/build.rs): on an older processor it
        // would stop the worker with an illegal instruction instead of saying why.
        if !cpu_ok() {
            return Err(FileError::new("old_processor"));
        }
        let mut whisper: Option<whisper::Whisper> = None;
        for f in &ctx.req.files {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || transcribe(ctx, f, &mut whisper));
        }
        Ok(())
    }
}

/// The sound of any audio or video as 16 kHz mono samples.
fn samples(ctx: &Ctx, path: &std::path::Path) -> Result<Vec<f32>, FileError> {
    let exe = ffmpeg::exe(ctx.req, "ffmpeg")?;
    let mut child = Command::new(exe)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"]).arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", "-"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| FileError::engine(format!("ffmpeg: {e}")))?;
    let mut raw = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut raw).map_err(super::io_err)?;
    let _ = child.wait();
    if raw.len() < 16000 * 4 / 10 {
        return Err(FileError::new("no_audio"));
    }
    Ok(raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

struct Segment {
    start: i64,
    end: i64,
    text: String,
}

/// The instructions crates/whisper/build.rs turns on (GGML_AVX2, FMA, F16C, BMI2).
fn cpu_ok() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
            && std::arch::is_x86_feature_detected!("bmi2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

fn transcribe(ctx: &Ctx, f: &FileRef, whisper: &mut Option<whisper::Whisper>) -> Result<Value, FileError> {
    let req = ctx.req;
    let path = f.input_path();
    let in_bytes = file_size(path);
    ctx.progress(&f.id, 0.0, "read", 1, 3);
    let audio = samples(ctx, path)?;
    ctx.check_cancel()?;
    if whisper.is_none() {
        ctx.progress(&f.id, 0.03, "model", 1, 3);
        let m = model(req);
        let file = req.engines.get(m.engine).map(|e| e.path.join(m.file)).filter(|p| p.is_file())
            .ok_or_else(|| FileError::with(codes::ENGINE_MISSING, json!({ "component": m.engine })))?;
        let w = whisper::Whisper::load(&file).ok_or_else(|| FileError::engine("whisper: the model could not be loaded"))?;
        *whisper = Some(w);
    }
    let w = whisper.as_mut().unwrap();
    let format = req.str_or("format", "txt");
    let prompt = req.str_or("prompt", "").trim().to_string();
    let opts = whisper::Options {
        language: req.str_or("language", "auto"),
        translate: req.bool_or("translate", false),
        threads: req.limits.threads.clamp(1, 16) as usize,
        // Short lines that end between words, like hand-made subtitles.
        max_len: if matches!(format, "srt" | "vtt") { 42 } else { 0 },
        beam_size: 5,
        prompt: &prompt,
    };
    ctx.progress(&f.id, 0.05, "transcribe", 2, 3);
    let started = std::time::Instant::now();
    let out = ctx.out.clone();
    let id = f.id.clone();
    let mut progress = |p: u32| out.progress(&id, 0.05 + 0.9 * p as f64 / 100.0, "transcribe", 2, 3, None);
    let stop = || ctx.cancel.is_cancelled();
    let fail = |e| match e {
        whisper::Error::Stopped => FileError::new(codes::CANCELLED),
        whisper::Error::Failed => FileError::engine("whisper: transcription failed"),
    };
    let (mut found, mut detected) = w.run(&audio, &opts, &mut progress, &stop).map_err(fail)?;
    // Whisper copies the style of the text before it: with nothing before, the large model
    // sometimes wrote a whole talk in lowercase without one full stop. Then it reads again
    // after a lone "." (no language of its own), which brings sentences back.
    let all: String = found.iter().map(|s| s.text.as_str()).collect();
    let letters = all.chars().filter(|c| c.is_alphanumeric()).count();
    if prompt.is_empty() && letters > 200 && !all.contains(['.', '!', '?', '。', '！', '？', '؟', '।']) {
        let opts = whisper::Options { prompt: ".", ..opts };
        // The bar is already near the end: it stays there rather than start over.
        (found, detected) = w.run(&audio, &opts, &mut |_| {}, &stop).map_err(fail)?;
    }
    eprintln!("transcribe: {:.0} s of sound in {:?}", audio.len() as f64 / 16000.0, started.elapsed());
    // Music or noise read as words: a "." or "♪" with nothing said is not a line.
    let segments: Vec<Segment> = found
        .into_iter()
        .filter(|s| s.text.chars().any(char::is_alphanumeric))
        .map(|s| Segment { start: s.start, end: s.end, text: s.text })
        .collect();
    if segments.is_empty() {
        return Err(FileError::new("no_speech"));
    }
    ctx.progress(&f.id, 0.97, "save", 3, 3);
    let body = match format {
        "srt" => srt(&segments).into_bytes(),
        "vtt" => vtt(&segments).into_bytes(),
        "docx" => docx(&text(&segments))?,
        _ => text(&segments).into_bytes(),
    };
    let dest = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
    write_file(&dest, &body)?;
    ctx.progress(&f.id, 1.0, "save", 3, 3);
    ctx.emit_output(&f.id, &dest, format, json!({ "language": detected }), in_bytes);
    Ok(Value::Null)
}

/// Plain text: a new paragraph where the speaker pauses for more than 1.5 s.
fn text(segs: &[Segment]) -> String {
    let mut out = String::new();
    let mut last_end = None;
    for s in segs {
        match last_end {
            Some(e) if s.start - e > 150 => out.push_str("\r\n\r\n"),
            Some(_) => out.push(' '),
            None => {}
        }
        out.push_str(&s.text);
        last_end = Some(s.end);
    }
    out.push_str("\r\n");
    out
}

/// A plain Word document, one paragraph per paragraph of the text.
fn docx(text: &str) -> Result<Vec<u8>, FileError> {
    use std::io::Write;
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let paras: String = text
        .split("\r\n\r\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", esc(p)))
        .collect();
    let files = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{paras}<w:sectPr/></w:body></w:document>"#)),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        zip.start_file(name, opts).map_err(FileError::engine)?;
        zip.write_all(body.as_bytes()).map_err(super::io_err)?;
    }
    Ok(zip.finish().map_err(FileError::engine)?.into_inner())
}

/// Whisper's times are in hundredths of a second.
fn stamp(cs: i64, sep: char) -> String {
    let ms = cs.max(0) * 10;
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn srt(segs: &[Segment]) -> String {
    segs.iter().enumerate().map(|(i, s)| format!("{}\r\n{} --> {}\r\n{}\r\n\r\n", i + 1, stamp(s.start, ','), stamp(s.end.max(s.start + 20), ','), s.text)).collect()
}

fn vtt(segs: &[Segment]) -> String {
    let mut out = String::from("WEBVTT\r\n\r\n");
    for s in segs {
        out.push_str(&format!("{} --> {}\r\n{}\r\n\r\n", stamp(s.start, '.'), stamp(s.end.max(s.start + 20), '.'), s.text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_processor_runs_whisper() {
        // The PCs that build and test MQKit (and GitHub's runners) all have AVX2.
        assert!(cpu_ok());
    }

    #[test]
    fn times() {
        assert_eq!(stamp(0, ','), "00:00:00,000");
        assert_eq!(stamp(366_012, ','), "01:01:00,120");
        let s = [Segment { start: 0, end: 250, text: "Bună.".into() }, Segment { start: 260, end: 400, text: "Da.".into() }, Segment { start: 700, end: 800, text: "Gata.".into() }];
        assert_eq!(text(&s), "Bună. Da.\r\n\r\nGata.\r\n");
        assert!(srt(&s).starts_with("1\r\n00:00:00,000 --> 00:00:02,500\r\nBună.\r\n\r\n2\r\n"));
    }
}
