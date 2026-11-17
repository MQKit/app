//! Fonts: TTF, OTF, WOFF and WOFF2 into each other, a web kit (WOFF2 with its CSS and a
//! page to try it), and smaller fonts that keep only the writing systems or the text
//! you need. Subsetting is HarfBuzz's (TrueType, CFF and variable fonts, with every
//! ligature, alternate and mark the kept letters need); WOFF2 is Google's encoder.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use skrifa::raw::{FileRef as FontFile, FontRef, TableProvider};
use skrifa::string::StringId;
use skrifa::{MetadataProvider, Tag};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use unicode_script::{Script, UnicodeScript};

pub const FONTS: &[&str] = &["ttf", "otf", "woff", "woff2", "ttc", "otc"];

pub struct Fonts;

impl Plugin for Fonts {
    fn id(&self) -> &'static str {
        "fonts"
    }

    fn manifest(&self) -> Value {
        let web = json!({ "field": "format", "in": ["woff2", "webkit", "woff"] });
        json!({
            "name": t("Fonturi", "Fonts"),
            "summary": t("TTF, OTF, WOFF2; fonturi mai mici", "TTF, OTF, WOFF2; smaller fonts"),
            "keywords": { "ro": ["font", "fonturi", "ttf", "otf", "woff", "woff2", "subset", "font web", "@font-face"],
                          "en": ["font", "fonts", "ttf", "otf", "woff", "woff2", "subset", "web font", "@font-face"] },
            "category": "other",
            "icon": "typography",
            "input": { "extensions": FONTS, "multiple": true, "folders": true, "mode": "combined" },
            "output": { "kind": "perInput", "extensions": ["woff2", "woff", "ttf", "otf", "folder"],
                        "nameVariables": [ { "id": "file", "label": t("numele fontului", "font name") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "family", "style", "faces", "chars", "scripts", "embedding", "web"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 2.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "display": "chips",
                  "default": [ { "when": { "file": "web", "eq": false, "quant": "any" }, "value": "woff2" }, { "value": "desktop" } ],
                  "options": [ opt("woff2", "WOFF2 (web)", "WOFF2 (web)"), opt("webkit", "Pachet web (WOFF2 + CSS)", "Web kit (WOFF2 + CSS)"),
                               opt("woff", "WOFF", "WOFF"), opt("desktop", "TTF / OTF (de instalat)", "TTF / OTF (to install)") ] },
                { "key": "keep", "type": "select", "label": t("Ce litere păstrezi", "Which letters to keep"), "default": "all",
                  "options": [ opt("all", "Toate", "All of them"), opt("scripts", "Doar anumite alfabete", "Only some writing systems"),
                               opt("text", "Doar literele dintr-un text", "Only the letters in a text") ],
                  "help": t("Un font cu mai puține litere e mult mai mic, deci o pagină web se încarcă mai repede.",
                            "A font with fewer letters is much smaller, so a web page loads faster.") },
                { "key": "scripts", "type": "multi", "label": t("Alfabete", "Writing systems"), "default": [],
                  "optionsFrom": { "file": "scripts" }, "showIf": { "field": "keep", "eq": "scripts" },
                  "help": t("Cifrele, semnele de punctuație și accentele rămân mereu.", "Digits, punctuation and accents always stay.") },
                { "key": "text", "type": "text", "label": t("Textul", "The text"), "default": "", "showIf": { "field": "keep", "eq": "text" },
                  "placeholder": t("Ex.: titlul unui logo", "E.g. a logo's title"),
                  "help": t("Rămân doar literele, cifrele și semnele din acest text (și spațiul).",
                            "Only the letters, digits and signs in this text stay (and the space).") },
                { "key": "noHinting", "type": "toggle", "default": false, "showIf": web,
                  "label": t("Fără hinting (încă mai mic)", "No hinting (smaller still)"),
                  "help": t("Hinting-ul ajută literele mici pe ecranele vechi; browserele moderne se descurcă și fără.",
                            "Hinting helps small letters on old screens; modern browsers do fine without it.") }
            ],
            "rules": [
                { "id": "pick_scripts", "severity": "error",
                  "when": { "all": [ { "field": "keep", "eq": "scripts" }, { "field": "scripts", "empty": true } ] },
                  "message": t("Alege cel puțin un alfabet.", "Choose at least one writing system.") },
                { "id": "type_text", "severity": "error",
                  "when": { "all": [ { "field": "keep", "eq": "text" }, { "field": "text", "eq": "" } ] },
                  "message": t("Scrie textul ale cărui litere le păstrezi.", "Type the text whose letters to keep.") },
                { "id": "restricted", "severity": "warning", "when": { "file": "embedding", "eq": "restricted", "quant": "any" },
                  "message": t("Licența unui font nu permite să fie pus în documente sau pe site-uri. Verifică licența înainte să-l folosești pe web.",
                               "A font's licence doesn't allow putting it in documents or on websites. Check the licence before using it on the web.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{file}", "{file}") } },
            "errors": {
                "not_font": t("Fișierul nu este un font pe care îl putem citi.", "This file isn't a font we can read."),
                "nothing_kept": t("Fontul nu are niciuna dintre literele alese.", "The font has none of the chosen letters."),
                "subset_failed": t("Fontul nu a putut fi micșorat.", "The font couldn't be made smaller."),
                "woff2_failed": t("Fontul nu a putut fi comprimat în WOFF2.", "The font couldn't be compressed to WOFF2.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let Ok(raw) = std::fs::read(f.input_path()) else { return Analysis::failed(codes::CORRUPT_FILE) };
        let Ok(sfnt) = to_sfnt(&raw) else { return Analysis::status("unsupported", "not_font") };
        let Ok(file) = FontFile::new(&sfnt) else { return Analysis::status("unsupported", "not_font") };
        let faces: Vec<FontRef> = file.fonts().flatten().collect();
        let Some(first) = faces.first() else { return Analysis::status("unsupported", "not_font") };
        let mut p = Map::new();
        let kind = match &raw[..4.min(raw.len())] {
            b"wOF2" => "woff2",
            b"wOFF" => "woff",
            b"ttcf" => "ttc",
            b"OTTO" => "otf",
            _ => "ttf",
        };
        p.insert("format".into(), json!(kind));
        p.insert("web".into(), json!(kind == "woff" || kind == "woff2"));
        p.insert("family".into(), json!(family(first)));
        p.insert("style".into(), json!(name(first, StringId::TYPOGRAPHIC_SUBFAMILY_NAME).or_else(|| name(first, StringId::SUBFAMILY_NAME))));
        p.insert("faces".into(), json!(faces.len()));
        let chars: Vec<u32> = codepoints(first);
        p.insert("chars".into(), json!(chars.len()));
        p.insert("scripts".into(), Value::Array(script_options(&chars)));
        let fs_type = first.os2().map(|o| o.fs_type()).unwrap_or(0);
        let embedding = match fs_type & 0xF {
            2 => "restricted",
            4 => "preview",
            8 => "editable",
            _ => "installable",
        };
        p.insert("embedding".into(), json!(embedding));
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        convert(ctx, f, None, &f.id, (0.0, 1.0))
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        if ctx.req.str_or("format", "woff2") == "webkit" {
            ctx.run_one("*", || web_kit(ctx));
        } else {
            for f in &ctx.req.files {
                if ctx.cancel.stop_after_current() {
                    break;
                }
                ctx.run_one(&f.id, || convert(ctx, f, None, &f.id, (0.0, 1.0)));
            }
        }
        Ok(())
    }
}

/// One finished face: its bytes in the target format and what the CSS needs.
struct Made {
    stem: String,
    ext: &'static str,
    family: String,
    weight: String,
    italic: bool,
    sample: String,
}

/// Converts every face of one file; emits each, or (for the web kit) writes them into `kit`.
/// Progress goes to `pid`, from `span.0` over `span.1` of the whole.
fn convert(ctx: &Ctx, f: &FileRef, kit: Option<(&Path, &mut Vec<Made>)>, pid: &str, span: (f64, f64)) -> Result<Value, FileError> {
    let req = ctx.req;
    let at = |x: f64| ctx.progress(pid, span.0 + span.1 * x, "convert", 1, 1);
    at(0.0);
    let path = f.input_path();
    let in_bytes = file_size(path);
    let raw = std::fs::read(path).map_err(super::io_err)?;
    let sfnt = to_sfnt(&raw)?;
    let file = FontFile::new(&sfnt).map_err(|_| FileError::new("not_font"))?;
    let faces: Vec<FontRef> = file.fonts().flatten().collect();
    if faces.is_empty() {
        return Err(FileError::new("not_font"));
    }
    let format = req.str_or("format", "woff2");
    let no_hinting = format != "desktop" && req.bool_or("noHinting", false);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "font".into());
    let mut kit = kit;
    let total = faces.len();
    for (i, face) in faces.iter().enumerate() {
        ctx.check_cancel()?;
        let step = |x: f64| at((i as f64 + x) / total as f64);
        step(0.1);
        let keep = kept(req, face)?;
        let plain = if keep.is_some() || no_hinting {
            let opts = hb_subset::Options { unicodes: keep.as_deref(), no_hinting };
            hb_subset::subset(&sfnt, i as u32, &opts).ok_or_else(|| FileError::new("subset_failed"))?
        } else if total > 1 {
            extract_face(&sfnt, i)?
        } else {
            sfnt.clone()
        };
        step(0.5);
        let cff = plain.starts_with(b"OTTO");
        let (bytes, ext) = match format {
            "woff" => (woff_encode(&plain)?, "woff"),
            "desktop" => (plain, if cff { "otf" } else { "ttf" }),
            _ => (woofwoof::compress(&plain, "", 11, true).ok_or_else(|| FileError::new("woff2_failed"))?, "woff2"),
        };
        step(0.9);
        let face_stem = if total > 1 {
            name(face, StringId::FULL_NAME).map(|n| safe_name(&n)).unwrap_or_else(|| format!("{stem}-{}", i + 1))
        } else {
            stem.clone()
        };
        match kit.as_mut() {
            Some((dir, made)) => {
                let mut name = face_stem.clone();
                let mut n = 2;
                while made.iter().any(|m| m.stem.eq_ignore_ascii_case(&name)) {
                    name = format!("{face_stem}-{n}");
                    n += 1;
                }
                write_file(&dir.join(format!("{name}.{ext}")), &bytes)?;
                let font = FontRef::from_index(&sfnt, i as u32).map_err(|_| FileError::new("not_font"))?;
                made.push(Made { stem: name, ext, family: family(&font), weight: weight(&font), italic: italic(&font), sample: sample(&font, keep.as_deref()) });
            }
            None => {
                let out = ctx.file_dir(&f.id)?.join(format!("{i}.{ext}"));
                write_file(&out, &bytes)?;
                ctx.emit_output(&f.id, &out, ext, json!({ "file": face_stem }), if i == 0 { in_bytes } else { 0 });
            }
        }
    }
    at(1.0);
    Ok(Value::Null)
}

/// All the fonts as WOFF2 in one folder, with fonts.css and a page to try them.
fn web_kit(ctx: &Ctx) -> Result<Value, FileError> {
    let dir = ctx.file_dir("kit")?.join("web");
    std::fs::create_dir_all(&dir).map_err(super::io_err)?;
    let mut made = Vec::new();
    let mut in_bytes = 0;
    let n = ctx.req.files.len().max(1) as f64;
    for (i, f) in ctx.req.files.iter().enumerate() {
        ctx.check_cancel()?;
        in_bytes += file_size(f.input_path());
        convert(ctx, f, Some((&dir, &mut made)), "*", (0.95 * i as f64 / n, 0.95 / n))?;
    }
    let mut css = String::new();
    let mut page = String::from("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>Fonts</title>\n<link rel=\"stylesheet\" href=\"fonts.css\">\n<style>\nbody { font: 14px system-ui, sans-serif; margin: 32px; color: #222; }\nsection { margin-bottom: 28px; }\nh2 { font-size: 13px; font-weight: 600; color: #666; margin: 0 0 6px; }\n.s { font-size: 36px; line-height: 1.25; outline: none; overflow-wrap: anywhere; }\n</style>\n</head>\n<body>\n");
    for m in &made {
        let style = if m.italic { "italic" } else { "normal" };
        css.push_str(&format!(
            "@font-face {{\n  font-family: \"{}\";\n  src: url(\"{}.{}\") format(\"{}\");\n  font-weight: {};\n  font-style: {};\n  font-display: swap;\n}}\n\n",
            m.family.replace('"', "'"), m.stem, m.ext, m.ext, m.weight, style
        ));
        page.push_str(&format!(
            "<section>\n<h2>{} · {} · {}</h2>\n<div class=\"s\" contenteditable spellcheck=\"false\" style=\"font-family: '{}'; font-weight: {}; font-style: {}\">{}</div>\n</section>\n",
            html(&m.family), html(&m.weight), style, html(&m.family.replace('\'', "\\'")), shown_weight(&m.weight), style, html(&m.sample)
        ));
    }
    page.push_str("</body>\n</html>\n");
    write_file(&dir.join("fonts.css"), css.trim_end().as_bytes())?;
    write_file(&dir.join("demo.html"), page.as_bytes())?;
    let families: std::collections::BTreeSet<&str> = made.iter().map(|m| m.family.as_str()).collect();
    let name = match families.iter().next() {
        Some(f) if families.len() == 1 => format!("{} web", safe_name(f)),
        _ => "fonts web".into(),
    };
    ctx.progress("*", 1.0, "convert", 1, 1);
    ctx.emit_folder("*", &dir, json!({ "file": name }), in_bytes);
    Ok(Value::Null)
}

/// The characters to keep, or None for all of them.
fn kept(req: &Request, face: &FontRef) -> Result<Option<Vec<u32>>, FileError> {
    let list: Vec<u32> = match req.str_or("keep", "all") {
        "scripts" => {
            let chosen: Vec<Script> = req.options.get("scripts").and_then(Value::as_array).into_iter().flatten()
                .filter_map(Value::as_str).filter_map(Script::from_short_name).collect();
            if chosen.is_empty() {
                return Ok(None);
            }
            codepoints(face).into_iter().filter(|&u| {
                char::from_u32(u).is_some_and(|c| {
                    let ext = c.script_extension();
                    chosen.iter().any(|s| ext.contains_script(*s))
                })
            }).collect()
        }
        "text" => {
            let have: std::collections::HashSet<u32> = codepoints(face).into_iter().collect();
            let mut want: Vec<u32> = req.str_or("text", "").chars().chain([' ', '\u{a0}']).map(|c| c as u32).collect();
            want.sort_unstable();
            want.dedup();
            want.retain(|u| have.contains(u));
            want
        }
        _ => return Ok(None),
    };
    if list.iter().all(|&u| u == 0x20 || u == 0xA0) {
        return Err(FileError::new("nothing_kept"));
    }
    Ok(Some(list))
}

fn codepoints(face: &FontRef) -> Vec<u32> {
    face.charmap().mappings().map(|(u, _)| u).collect()
}

/// The writing systems a font covers well enough to choose from (most letters first).
fn script_options(chars: &[u32]) -> Vec<Value> {
    let mut count: BTreeMap<&'static str, usize> = BTreeMap::new();
    for c in chars.iter().filter_map(|&u| char::from_u32(u)) {
        let s = c.script();
        if !matches!(s, Script::Common | Script::Inherited | Script::Unknown) {
            *count.entry(s.short_name()).or_default() += 1;
        }
    }
    let mut list: Vec<(&str, usize)> = count.into_iter().filter(|(_, n)| *n >= 10).collect();
    list.sort_by_key(|x| std::cmp::Reverse(x.1));
    list.into_iter().map(|(code, _)| {
        let (ro, en) = script_name(code);
        json!({ "value": code, "label": t(&ro, &en) })
    }).collect()
}

fn script_name(code: &str) -> (String, String) {
    let known: &[(&str, &str, &str)] = &[
        ("Latn", "Latin", "Latin"), ("Cyrl", "Chirilic", "Cyrillic"), ("Grek", "Grec", "Greek"), ("Arab", "Arab", "Arabic"),
        ("Hebr", "Ebraic", "Hebrew"), ("Armn", "Armean", "Armenian"), ("Geor", "Georgian", "Georgian"), ("Deva", "Devanagari", "Devanagari"),
        ("Beng", "Bengali", "Bengali"), ("Guru", "Gurmukhi", "Gurmukhi"), ("Gujr", "Gujarati", "Gujarati"), ("Orya", "Odia", "Odia"),
        ("Taml", "Tamil", "Tamil"), ("Telu", "Telugu", "Telugu"), ("Knda", "Kannada", "Kannada"), ("Mlym", "Malayalam", "Malayalam"),
        ("Sinh", "Singalez", "Sinhala"), ("Thai", "Thailandez", "Thai"), ("Laoo", "Laoțian", "Lao"), ("Tibt", "Tibetan", "Tibetan"),
        ("Mymr", "Birman", "Myanmar"), ("Khmr", "Khmer", "Khmer"), ("Hang", "Coreean (Hangul)", "Korean (Hangul)"),
        ("Hani", "Han (chinez, japonez kanji)", "Han (Chinese, Japanese kanji)"), ("Hira", "Hiragana", "Hiragana"), ("Kana", "Katakana", "Katakana"),
        ("Ethi", "Etiopian", "Ethiopic"), ("Thaa", "Thaana", "Thaana"), ("Syrc", "Siriac", "Syriac"), ("Mong", "Mongol", "Mongolian"),
        ("Cher", "Cherokee", "Cherokee"), ("Copt", "Copt", "Coptic"), ("Runr", "Runic", "Runic"), ("Tfng", "Tifinagh", "Tifinagh"),
        ("Cans", "Silabar canadian", "Canadian syllabics"), ("Bopo", "Bopomofo", "Bopomofo"),
    ];
    match known.iter().find(|k| k.0 == code) {
        Some((_, ro, en)) => (ro.to_string(), en.to_string()),
        None => {
            let full = Script::from_short_name(code).map(|s| s.full_name().replace('_', " ")).unwrap_or_else(|| code.into());
            (full.clone(), full)
        }
    }
}

fn name(face: &FontRef, id: StringId) -> Option<String> {
    face.localized_strings(id).english_or_first().map(|s| s.to_string()).filter(|s| !s.trim().is_empty())
}

fn family(face: &FontRef) -> String {
    name(face, StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| name(face, StringId::FAMILY_NAME)).unwrap_or_else(|| "Font".into())
}

/// "700", or "100 900" for a variable font with a weight axis.
fn weight(face: &FontRef) -> String {
    if let Some(axis) = face.axes().get_by_tag(Tag::new(b"wght")) {
        return format!("{} {}", axis.min_value().round(), axis.max_value().round());
    }
    format!("{}", face.attributes().weight.value().round())
}

/// The weight to show a face at: its own, or regular (400) inside a variable range.
fn shown_weight(w: &str) -> String {
    let n: Vec<f32> = w.split(' ').filter_map(|x| x.parse().ok()).collect();
    match n.as_slice() {
        [lo, hi] => format!("{}", 400f32.clamp(*lo, *hi)),
        _ => w.to_string(),
    }
}

fn italic(face: &FontRef) -> bool {
    !matches!(face.attributes().style, skrifa::attribute::Style::Normal)
}

/// Letters to show on the try-it page: a few of each writing system kept, then the digits.
fn sample(face: &FontRef, keep: Option<&[u32]>) -> String {
    let list = keep.map(|k| k.to_vec()).unwrap_or_else(|| codepoints(face));
    let mut chars: Vec<char> = list.iter().filter_map(|&u| char::from_u32(u)).collect();
    chars.sort_by_key(|c| !c.is_lowercase());
    let mut by_script: Vec<(Script, String)> = Vec::new();
    for &c in chars.iter().filter(|c| c.is_alphabetic()) {
        let s = c.script();
        if matches!(s, Script::Common | Script::Inherited | Script::Unknown) {
            continue;
        }
        match by_script.iter_mut().find(|(k, _)| *k == s) {
            Some((_, text)) if text.chars().count() < 26 => text.push(c),
            Some(_) => {}
            None => by_script.push((s, c.to_string())),
        }
    }
    let mut parts: Vec<String> = by_script.into_iter().map(|(_, t)| t).collect();
    let digits: String = chars.iter().filter(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        parts.push(digits);
    }
    let out = parts.join(" ");
    if out.trim().is_empty() {
        chars.iter().filter(|c| !c.is_whitespace() && !c.is_control()).take(40).collect()
    } else {
        out
    }
}

/// A name that is fine as a Windows file name.
fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '-' } else { c }).collect();
    let t = t.trim().trim_end_matches('.').to_string();
    if t.is_empty() { "font".into() } else { t }
}

fn html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- the font container formats ----

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Any input as a plain TrueType/OpenType font or collection.
fn to_sfnt(raw: &[u8]) -> Result<Vec<u8>, FileError> {
    match raw.get(..4) {
        Some(b"wOF2") => woofwoof::decompress(raw).ok_or_else(|| FileError::new("not_font")),
        Some(b"wOFF") => woff_decode(raw).ok_or_else(|| FileError::new("not_font")),
        Some([0, 1, 0, 0]) | Some(b"OTTO") | Some(b"true") | Some(b"ttcf") => Ok(raw.to_vec()),
        _ => Err(FileError::new("not_font")),
    }
}

/// The tables of the font whose table directory starts at `at`.
type Tables<'a> = Vec<([u8; 4], u32, &'a [u8])>;

fn tables(d: &[u8], at: usize) -> Option<(u32, Tables<'_>)> {
    let flavor = be32(d, at)?;
    let n = be16(d, at + 4)? as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let r = at + 12 + i * 16;
        let tag: [u8; 4] = d.get(r..r + 4)?.try_into().ok()?;
        let sum = be32(d, r + 4)?;
        let off = be32(d, r + 8)? as usize;
        let len = be32(d, r + 12)? as usize;
        out.push((tag, sum, d.get(off..off.checked_add(len)?)?));
    }
    Some((flavor, out))
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |s, c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(b))
    })
}

/// A plain font from its tables (sorted, padded, checksums and head adjustment set).
fn build_sfnt(flavor: u32, mut list: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    list.sort_by_key(|t| t.0);
    let n = list.len() as u16;
    let pow = if n == 0 { 0 } else { 15 - n.leading_zeros() as u16 };
    let range = (1u16 << pow) * 16;
    let mut out = Vec::new();
    out.extend(flavor.to_be_bytes());
    out.extend(n.to_be_bytes());
    out.extend(range.to_be_bytes());
    out.extend(pow.to_be_bytes());
    out.extend((n * 16 - range).to_be_bytes());
    let mut off = 12 + 16 * list.len();
    let mut head_at = None;
    for (tag, data) in list.iter_mut() {
        if tag == b"head" && data.len() >= 12 {
            data[8..12].fill(0);
            head_at = Some(off);
        }
        out.extend(tag.iter());
        out.extend(checksum(data).to_be_bytes());
        out.extend((off as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        off += (data.len() + 3) & !3;
    }
    for (_, data) in &list {
        out.extend(data.iter());
        out.resize((out.len() + 3) & !3, 0);
    }
    if let Some(h) = head_at {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[h + 8..h + 12].copy_from_slice(&adj.to_be_bytes());
    }
    out
}

/// One face of a collection as a font of its own.
fn extract_face(d: &[u8], index: usize) -> Result<Vec<u8>, FileError> {
    let bad = || FileError::new("not_font");
    let at = be32(d, 12 + index * 4).ok_or_else(bad)? as usize;
    let (flavor, list) = tables(d, at).ok_or_else(bad)?;
    Ok(build_sfnt(flavor, list.into_iter().map(|(tag, _, data)| (tag, data.to_vec())).collect()))
}

/// WOFF 1.0: each table zlib-compressed.
fn woff_decode(d: &[u8]) -> Option<Vec<u8>> {
    let flavor = be32(d, 4)?;
    let n = be16(d, 12)? as usize;
    let mut list = Vec::with_capacity(n);
    for i in 0..n {
        let r = 44 + i * 20;
        let tag: [u8; 4] = d.get(r..r + 4)?.try_into().ok()?;
        let off = be32(d, r + 4)? as usize;
        let comp = be32(d, r + 8)? as usize;
        let orig = be32(d, r + 12)? as usize;
        let src = d.get(off..off.checked_add(comp)?)?;
        let data = if comp < orig {
            let mut v = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(src).take(orig as u64).read_to_end(&mut v).ok()?;
            if v.len() != orig {
                return None;
            }
            v
        } else {
            src.to_vec()
        };
        list.push((tag, data));
    }
    Some(build_sfnt(flavor, list))
}

fn woff_encode(sfnt: &[u8]) -> Result<Vec<u8>, FileError> {
    let (flavor, list) = tables(sfnt, 0).ok_or_else(|| FileError::new("not_font"))?;
    let mut list = list;
    list.sort_by_key(|t| t.0);
    let n = list.len();
    let sfnt_size: usize = 12 + 16 * n + list.iter().map(|t| (t.2.len() + 3) & !3).sum::<usize>();
    let mut body = Vec::new();
    let mut dir = Vec::new();
    let mut off = 44 + 20 * n;
    for (tag, sum, data) in &list {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(data).map_err(super::io_err)?;
        let packed = z.finish().map_err(super::io_err)?;
        let stored: &[u8] = if packed.len() < data.len() { &packed } else { data };
        dir.extend(tag.iter());
        dir.extend((off as u32).to_be_bytes());
        dir.extend((stored.len() as u32).to_be_bytes());
        dir.extend((data.len() as u32).to_be_bytes());
        dir.extend(sum.to_be_bytes());
        body.extend(stored);
        let padded = (stored.len() + 3) & !3;
        body.resize(body.len() + padded - stored.len(), 0);
        off += padded;
    }
    let total = 44 + dir.len() + body.len();
    let (major, minor) = list.iter().find(|t| &t.0 == b"head").and_then(|t| Some((be16(t.2, 4)?, be16(t.2, 6)?))).unwrap_or((1, 0));
    let mut out = Vec::with_capacity(total);
    out.extend(b"wOFF");
    out.extend(flavor.to_be_bytes());
    out.extend((total as u32).to_be_bytes());
    out.extend((n as u16).to_be_bytes());
    out.extend(0u16.to_be_bytes());
    out.extend((sfnt_size as u32).to_be_bytes());
    out.extend(major.to_be_bytes());
    out.extend(minor.to_be_bytes());
    out.extend([0u8; 20]);
    out.extend(dir);
    out.extend(body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn woff_round_trip() {
        let font = std::fs::read("C:/Windows/Fonts/arial.ttf").unwrap();
        let w = woff_encode(&font).unwrap();
        assert!(w.len() < font.len());
        let back = woff_decode(&w).unwrap();
        let (_, a) = tables(&font, 0).unwrap();
        let (_, b) = tables(&back, 0).unwrap();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.0, y.0);
            if &x.0 != b"head" {
                assert_eq!(x.2, y.2);
            }
        }
    }

    #[test]
    fn subset_keeps_letters() {
        let font = std::fs::read("C:/Windows/Fonts/arial.ttf").unwrap();
        let out = hb_subset::subset(&font, 0, &hb_subset::Options { unicodes: Some(&[0x41, 0x42, 0x20]), no_hinting: false }).unwrap();
        assert!(out.len() < font.len() / 10);
        let f = FontRef::new(&out).unwrap();
        let mut got = codepoints(&f);
        got.sort();
        assert_eq!(got, vec![0x20, 0x41, 0x42]);
    }
}
