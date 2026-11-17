//! Text to speech with the voices Windows already has: a text, Markdown or Word file
//! read aloud into WAV or MP3. Long texts are read in pieces and joined into one file.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{ffmpeg, winrt};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::Path;

pub struct Tts;

impl Plugin for Tts {
    fn id(&self) -> &'static str {
        "tts"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Text în voce", "Text to speech"),
            "summary": t("Ascultă-ți textele", "Listen to your texts"),
            "keywords": { "ro": ["citește", "voce", "audio", "carte audio", "mp3"], "en": ["read aloud", "voice", "audio", "audiobook", "mp3"] },
            "category": "audio",
            "icon": "speakerphone",
            "requires": [ { "engine": "ffmpeg", "when": { "field": "format", "eq": "mp3" } } ],
            "input": { "extensions": ["txt", "md", "markdown", "docx"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["mp3", "wav"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["characters"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 5.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "voice", "type": "select", "label": t("Vocea", "Voice"), "default": "", "optionsFrom": { "env": "windows.voices" },
                  "help": t("Vocile vin din Windows; poți adăuga gratuit voci în alte limbi.", "Voices come from Windows; you can add voices in other languages for free."),
                  "helpLink": { "label": t("Adaugă voci în Setări", "Add voices in Settings"), "url": "ms-settings:speech" } },
                { "key": "rate", "type": "select", "label": t("Viteză", "Speed"), "default": 1,
                  "options": [ opt(0.75, "Mai rar", "Slower"), opt(1, "Normal", "Normal"), opt(1.25, "Puțin mai repede", "A little faster"),
                               opt(1.5, "Repede", "Fast") ] },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "mp3", "display": "chips",
                  "options": [ opt("mp3", "MP3", "MP3"), opt("wav", "WAV", "WAV") ] }
            ],
            "rules": [
                { "id": "no_own_voice", "severity": "info", "when": { "env": "windows.voiceOwnLanguage", "eq": false },
                  "message": t("Pe acest calculator nu e instalată o voce în limba ta. O poți adăuga gratuit din Setările Windows (Vorbire › Adaugă voci).",
                               "No voice in your language is installed on this PC. You can add one for free in Windows Settings (Speech › Add voices).") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "empty_text": t("Fișierul nu conține text.", "The file has no text."),
                "no_voices": t("Windows nu are nicio voce instalată.", "Windows has no voice installed.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let mut p = Map::new();
        p.insert("characters".into(), json!(read_text(f.input_path()).map(|t| t.chars().count()).unwrap_or(0)));
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "speak", 1, 2);
        let text = read_text(path)?;
        if text.trim().is_empty() {
            return Err(FileError::new("empty_text"));
        }
        let voices = winrt::voices()?;
        if voices.is_empty() {
            return Err(FileError::new("no_voices"));
        }
        let voice = match req.str_or("voice", "") {
            "" => None,
            v => Some(v.to_string()),
        };
        let rate = req.f64_or("rate", 1.0);
        let pieces = chunks(&text, 1500);
        let n = pieces.len();
        let mut fmt: Option<Vec<u8>> = None;
        let mut pcm: Vec<u8> = Vec::new();
        for (i, piece) in pieces.iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, 0.05 + 0.8 * i as f64 / n as f64, "speak", 1, 2);
            let wav = winrt::speak(piece, voice.as_deref(), rate)?;
            let (chunk_fmt, data) = wav_parts(&wav).ok_or_else(|| FileError::with(codes::ENGINE_FAILED, json!({ "message": "bad WAV from Windows" })))?;
            fmt.get_or_insert_with(|| chunk_fmt.to_vec());
            pcm.extend_from_slice(data);
        }
        let wav = build_wav(&fmt.unwrap_or_default(), &pcm);
        let dir = ctx.file_dir(&f.id)?;
        let format = req.str_or("format", "mp3");
        ctx.progress(&f.id, 0.88, "encode", 2, 2);
        let out = if format == "wav" {
            let out = dir.join("result.wav");
            write_file(&out, &wav)?;
            out
        } else {
            std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
            let tmp = ctx.work_dir.join(format!("speech-{}.wav", f.id));
            write_file(&tmp, &wav)?;
            let out = dir.join("result.mp3");
            let a: Vec<String> = vec!["-i".into(), tmp.to_string_lossy().into_owned(), "-c:a".into(), "libmp3lame".into(), "-b:a".into(), "96k".into(), out.to_string_lossy().into_owned()];
            ffmpeg::run(ctx, &f.id, &a, 0.0, 2, 2)?;
            out
        };
        ctx.progress(&f.id, 1.0, "encode", 2, 2);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(json!({ "pieces": n }))
    }
}

/// The text of a .txt, .md (without the Markdown marks) or .docx file.
fn read_text(path: &Path) -> Result<String, FileError> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "docx" {
        let file = std::fs::File::open(path).map_err(super::io_err)?;
        let mut z = zip::ZipArchive::new(file).map_err(|e| FileError::with(codes::CORRUPT_FILE, json!({ "message": e.to_string() })))?;
        let mut xml = String::new();
        z.by_name("word/document.xml").map_err(|_| FileError::new(codes::CORRUPT_FILE))?.read_to_string(&mut xml).map_err(super::io_err)?;
        return Ok(docx_text(&xml));
    }
    let raw = std::fs::read(path).map_err(super::io_err)?;
    let text = crate::common::text::decode(&raw);
    Ok(if ext == "md" || ext == "markdown" { strip_markdown(&text) } else { text })
}

/// Paragraph text of a Word document: `<w:t>` runs, a break after each `</w:p>`.
fn docx_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        let Some(j) = rest[i..].find('>') else { break };
        let tag = &rest[i..i + j + 1];
        rest = &rest[i + j + 1..];
        // Exactly <w:t> or <w:t …>: not <w:tbl>, <w:tc>, <w:tr>, <w:tab/>.
        if (tag == "<w:t>" || tag.starts_with("<w:t ")) && !tag.ends_with("/>") {
            if let Some(end) = rest.find("</w:t>") {
                out.push_str(&unescape(&rest[..end]));
                rest = &rest[end..];
            }
        } else if tag == "</w:p>" {
            out.push_str("\n\n");
        } else if tag.starts_with("<w:tab") || tag.starts_with("<w:br") {
            out.push(' ');
        }
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// What a person would read aloud from Markdown: headings, paragraphs, list items, table
/// cells, link text and picture descriptions. Not the front matter, web addresses, raw
/// HTML (scripts included), code blocks or footnote marks, which were read out before.
fn strip_markdown(s: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let (body, _) = super::markdown::front_matter(s);
    let opts = Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = String::new();
    let mut in_code = false;
    // A pause where a block ends: a heading has no full stop of its own.
    let end_block = |out: &mut String, sentence: bool| {
        let t = out.trim_end().len();
        out.truncate(t);
        if sentence && !out.is_empty() && !out.ends_with(['.', '!', '?', ':', ';', '…']) {
            out.push('.');
        }
        out.push_str("\n\n");
    };
    for ev in Parser::new_ext(&body, opts) {
        match ev {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            _ if in_code => {}
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),
            Event::End(TagEnd::Heading(_)) | Event::End(TagEnd::TableRow) | Event::End(TagEnd::TableHead) => end_block(&mut out, true),
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) | Event::End(TagEnd::BlockQuote(_)) | Event::Rule => end_block(&mut out, false),
            Event::End(TagEnd::TableCell) => out.push_str(", "),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Pieces of at most about `max` characters, cut at paragraph or sentence ends.
fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for sentence in text.split_inclusive(['.', '!', '?', '\n']) {
        if cur.chars().count() + sentence.chars().count() > max && !cur.trim().is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(sentence);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// (fmt chunk body, data chunk body) of a RIFF WAV.
fn wav_parts(w: &[u8]) -> Option<(&[u8], &[u8])> {
    if w.len() < 12 || &w[0..4] != b"RIFF" || &w[8..12] != b"WAVE" {
        return None;
    }
    let (mut fmt, mut data) = (None, None);
    let mut i = 12;
    while i + 8 <= w.len() {
        let id = &w[i..i + 4];
        let len = u32::from_le_bytes([w[i + 4], w[i + 5], w[i + 6], w[i + 7]]) as usize;
        let body = w.get(i + 8..(i + 8 + len).min(w.len()))?;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    Some((fmt?, data?))
}

fn build_wav(fmt: &[u8], data: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&((4 + 8 + fmt.len() + 8 + data.len()) as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    w.extend_from_slice(fmt);
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(data);
    w
}

#[cfg(test)]
mod tests {
    #[test]
    fn markdown_read_aloud() {
        let md = "---\ntitle: X\n---\n# Capitol\n\nText **bold** [link](https://example.com)[^1].\n\n```py\ncode()\n```\n\n<script>alert(1)</script>\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n![poză](a.png)\n\n[^1]: Nota.\n";
        let t = super::strip_markdown(md);
        assert!(t.starts_with("Capitol.\n\nText bold link."), "{t}");
        for bad in ["title", "https", "code()", "script", "a.png", "^1"] {
            assert!(!t.contains(bad), "{bad} in {t}");
        }
        assert!(t.contains("1, 2") && t.contains("poză") && t.contains("Nota."), "{t}");
    }
}
