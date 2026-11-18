//! QR codes from text, links and contacts: drawn as vectors (square, rounded, dots or
//! flowing modules, styled corner eyes, one color or a gradient), with a logo in the
//! middle if wanted and a caption under it. Each code is read back after drawing, so a
//! code that a phone might not read (a logo too big, colors too pale) is said.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{img, text::decode};
use contract::{codes, opt, t, FileError, FileRef, Request};
use qrcodegen::{QrCode, QrCodeEcc, QrSegment};
use serde_json::{json, Map, Value};
use std::fmt::Write as _;
use std::path::Path;

pub struct QrCodes;

/// The most a code holds, in bytes of UTF-8 (version 40, lowest correction).
const MAX_BYTES: usize = 2953;

impl Plugin for QrCodes {
    fn id(&self) -> &'static str {
        "qr-code"
    }

    fn manifest(&self) -> Value {
        let not_svg = json!({ "field": "format", "ne": "svg" });
        let two_colors = json!({ "field": "fill", "ne": "solid" });
        json!({
            "name": t("Coduri QR", "QR codes"),
            "summary": t("Frumoase, cu logo", "Styled, with a logo"),
            "keywords": { "ro": ["qr", "cod qr", "generator qr", "logo", "link"], "en": ["qr", "qr code", "qr generator", "logo", "link"] },
            "category": "image",
            "icon": "qrcode",
            "input": { "extensions": ["txt", "csv", "vcf"], "multiple": true, "folders": false, "mode": "perFile", "textEntry": "txt", "textFirst": true,
                       "textPlaceholder": t("Un link sau un text, de exemplu https://mqkit.com. Pentru mai multe coduri, câte unul pe rând, cu „Un cod pe rând”.",
                                            "A link or some text, for example https://mqkit.com. For several codes, one per line, with “A code per line”.") },
            "output": { "kind": "multiPerInput", "extensions": ["png", "svg", "jpg"],
                        "nameVariables": [ { "id": "text", "label": t("începutul textului", "start of the text") }, { "id": "n", "label": t("numărul codului", "code number") } ],
                        "replaceOriginalAllowed": false },
            "analysis": { "fast": ["codes", "longest"] },
            "ui": { "preview": "result", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [
                { "id": "fluid", "label": t("Rotunjit", "Rounded"), "default": true,
                  "values": { "modules": "fluid", "eyes": "rounded", "fill": "solid", "color": "#1b1f2a" } },
                { "id": "dots", "label": t("Puncte", "Dots"), "values": { "modules": "dots", "eyes": "circle", "fill": "solid", "color": "#1b1f2a" } },
                { "id": "gradient", "label": t("Colorat", "Colorful"),
                  "values": { "modules": "fluid", "eyes": "rounded", "fill": "linear", "color": "#3a2fd6", "color2": "#d6338f" } },
                { "id": "classic", "label": t("Clasic", "Classic"), "values": { "modules": "square", "eyes": "square", "fill": "solid", "color": "#000000" } }
            ],
            "groups": [ { "id": "look", "label": t("Aspect", "Look"), "order": 1 }, { "id": "more", "label": t("Detalii", "Details"), "order": 2 } ],
            "fields": [
                { "key": "split", "type": "select", "label": t("Din fiecare fișier", "From each file"), "default": "whole", "display": "chips",
                  "options": [ opt("whole", "Un cod", "One code"), opt("lines", "Un cod pe rând", "A code per line") ],
                  "help": t("„Un cod pe rând” face câte un cod pentru fiecare rând, de exemplu dintr-o listă de linkuri.",
                            "“A code per line” makes a code for each line, for example from a list of links.") },
                { "key": "modules", "type": "select", "label": t("Punctele", "The dots"), "default": "fluid", "display": "chips",
                  "options": [ opt("square", "Pătrate", "Squares"), opt("rounded", "Rotunjite", "Rounded"), opt("fluid", "Unite", "Joined"), opt("dots", "Puncte", "Dots") ] },
                { "key": "eyes", "type": "select", "label": t("Colțurile", "The corners"), "default": "rounded", "display": "chips",
                  "options": [ opt("square", "Pătrate", "Square"), opt("rounded", "Rotunjite", "Rounded"), opt("circle", "Cercuri", "Circles") ] },
                { "key": "color", "type": "color", "label": t("Culoare", "Color"), "default": "#1b1f2a" },
                { "key": "fill", "type": "select", "label": t("Umplere", "Fill"), "default": "solid", "display": "chips",
                  "options": [ opt("solid", "O culoare", "One color"), opt("linear", "Degradeu", "Gradient"), opt("radial", "Degradeu rotund", "Round gradient") ] },
                { "key": "color2", "type": "color", "label": t("A doua culoare", "Second color"), "default": "#d6338f", "showIf": two_colors },
                { "key": "eyeColorOn", "type": "toggle", "group": "look", "advanced": true, "default": false, "label": t("Colțurile în altă culoare", "Corners in another color") },
                { "key": "eyeColor", "type": "color", "group": "look", "advanced": true, "default": "#d6338f", "label": t("Culoarea colțurilor", "Corner color"),
                  "showIf": { "field": "eyeColorOn", "eq": true } },
                { "key": "background", "type": "select", "label": t("Fundal", "Background"), "default": "color", "display": "chips",
                  "options": [ opt("color", "Culoare", "Color"), opt("transparent", "Transparent", "Transparent") ] },
                { "key": "backgroundColor", "type": "color", "label": t("Culoarea fundalului", "Background color"), "default": "#ffffff",
                  "showIf": { "field": "background", "eq": "color" } },
                { "key": "logo", "type": "file", "label": t("Logo în mijloc (opțional)", "Logo in the middle (optional)"),
                  "accept": ["png", "jpg", "jpeg", "webp", "svg"] },
                { "key": "logoSize", "type": "range", "label": t("Mărimea logoului", "Logo size"), "min": 10, "max": 30, "step": 1, "unit": "%", "default": 22,
                  "showIf": { "field": "logo", "empty": false } },
                { "key": "logoPlate", "type": "toggle", "default": true, "label": t("Fundal sub logo", "Background under the logo"),
                  "showIf": { "field": "logo", "empty": false } },
                { "key": "caption", "type": "text", "default": "", "label": t("Text sub cod (opțional)", "Text under the code (optional)"),
                  "placeholder": t("Scanează-mă", "Scan me") },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "png", "display": "chips",
                  "options": [ opt("png", "PNG", "PNG"), opt("svg", "SVG (pentru tipar)", "SVG (for print)"), opt("jpg", "JPG", "JPG") ] },
                { "key": "size", "type": "select", "label": t("Mărime", "Size"), "default": 1024, "showIf": not_svg,
                  "options": [ opt(512, "512 px", "512 px"), opt(1024, "1024 px", "1024 px"), opt(2048, "2048 px", "2048 px"), opt(4096, "4096 px (tipar)", "4096 px (print)") ] },
                { "key": "margin", "type": "range", "group": "more", "advanced": true, "min": 0, "max": 8, "step": 1, "default": 4,
                  "label": t("Margine (în puncte)", "Margin (in dots)"),
                  "help": t("Telefoanele citesc cel mai sigur cu 4. Mai puțin doar dacă e spațiu liber în jur.", "Phones read best with 4. Less only if there is empty space around it.") },
                { "key": "ecc", "type": "select", "group": "more", "advanced": true, "default": "auto", "label": t("Rezistență la deteriorare", "Damage tolerance"),
                  "options": [ opt("auto", "Automat (mare cu logo)", "Automatic (high with a logo)"), opt("L", "Mică (7%)", "Low (7%)"),
                               opt("M", "Medie (15%)", "Medium (15%)"), opt("Q", "Mare (25%)", "High (25%)"), opt("H", "Maximă (30%)", "Highest (30%)") ] }
            ],
            "rules": [
                { "id": "too_long", "severity": "error", "when": { "file": "longest", "gt": MAX_BYTES, "quant": "any" },
                  "message": t("Un text e prea lung pentru un cod QR (cel mult 2953 de litere simple). Fă câte un cod pe rând sau scurtează-l.",
                               "A text is too long for a QR code (2,953 plain letters at most). Make a code per line, or shorten it.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{text}", "{text}") } },
            "errors": {
                "empty_text": t("Fișierul nu are text.", "The file has no text."),
                "too_long": t("Textul e prea lung pentru un cod QR.", "The text is too long for a QR code."),
                "bad_logo": t("Logoul nu a putut fi deschis.", "The logo couldn't be opened."),
                "may_not_scan": t("Codul s-ar putea să nu fie citit de toate telefoanele: micșorează logoul sau alege culori cu mai mult contrast.",
                                  "Some phones may not read the code: make the logo smaller or choose colors with more contrast.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let Ok(raw) = std::fs::read(f.input_path()) else { return Analysis::failed(codes::UNSUPPORTED_FILE) };
        let items = contents(&decode(&raw), f.input_path(), req.str_or("split", "whole"));
        if items.is_empty() {
            return Analysis::failed("empty_text");
        }
        let mut p = Map::new();
        p.insert("codes".into(), json!(items.len()));
        p.insert("longest".into(), json!(items.iter().map(|s| s.len()).max().unwrap_or(0)));
        Analysis::ok(p)
    }

    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        let Some(f) = super::preview_file(req) else { return Ok(None) };
        let raw = std::fs::read(f.input_path()).map_err(super::io_err)?;
        let Some(first) = contents(&decode(&raw), f.input_path(), req.str_or("split", "whole")).into_iter().next() else { return Ok(None) };
        let max = req.target.as_ref().and_then(|t| t.get("maxPx")).and_then(Value::as_u64).unwrap_or(480).clamp(64, 1600) as u32;
        let style = Style::from(req)?;
        let svg = draw(&encode(&first, &style)?, &style);
        let png = raster(&svg, max, &style, false)?;
        crate::common::preview::write_single(req, &png, "png").map(Some)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        let raw = std::fs::read(path).map_err(super::io_err)?;
        let items = contents(&decode(&raw), path, req.str_or("split", "whole"));
        if items.is_empty() {
            return Err(FileError::new("empty_text"));
        }
        let style = Style::from(req)?;
        let format = req.str_or("format", "png");
        let size = req.f64_or("size", 1024.0).clamp(128.0, 8192.0) as u32;
        let dir = ctx.file_dir(&f.id)?;
        let n = items.len();
        for (i, text) in items.iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, i as f64 / n as f64, "render", 1, 1);
            let qr = encode(text, &style)?;
            let svg = draw(&qr, &style);
            if !scannable(&qr, &style, text)? {
                ctx.out.warning(Some(&f.id), "may_not_scan", Some(json!({ "n": i + 1 })));
            }
            let (data, ext) = match format {
                "svg" => (svg.into_bytes(), "svg"),
                "jpg" => {
                    let png = raster(&svg, size, &style, true)?;
                    let im = image::load_from_memory(&png).map_err(img::img_err)?;
                    (img::encode_jpeg(&im, 92, None, None, [255, 255, 255])?, "jpg")
                }
                _ => (raster(&svg, size, &style, false)?, "png"),
            };
            let out = dir.join(format!("qr-{}.{ext}", i + 1));
            write_file(&out, &data)?;
            ctx.emit_output(&f.id, &out, ext, json!({ "text": name_of(text, i + 1), "n": i + 1 }), if i == 0 { in_bytes } else { 0 });
        }
        ctx.progress(&f.id, 1.0, "render", 1, 1);
        Ok(Value::Null)
    }
}

/// What to encode: the whole text (a contact card, a message), or one item per line.
/// A CSV gives the first cell of each row.
fn contents(text: &str, path: &Path, split: &str) -> Vec<String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let text = text.replace("\r\n", "\n");
    if ext == "vcf" {
        // A card as phones expect it: lines ended with CRLF.
        let card = text.trim().replace('\n', "\r\n");
        return if card.is_empty() { vec![] } else { vec![card] };
    }
    if ext == "csv" || split == "lines" {
        return text
            .lines()
            .map(|l| if ext == "csv" { first_cell(l) } else { l.trim().to_string() })
            .filter(|l| !l.is_empty())
            .collect();
    }
    let whole = text.trim().to_string();
    if whole.is_empty() { vec![] } else { vec![whole] }
}

fn first_cell(line: &str) -> String {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('"') {
        // "a ""quoted"" cell",…
        let mut out = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    out.push('"');
                    chars.next();
                } else {
                    break;
                }
            } else {
                out.push(c);
            }
        }
        return out.trim().to_string();
    }
    line.split([',', ';', '\t']).next().unwrap_or("").trim().to_string()
}

/// A short, file-safe name from the text: "https://www.mqkit.com/x" → "mqkit.com-x".
fn name_of(text: &str, n: usize) -> String {
    let mut s = text.lines().next().unwrap_or("").trim();
    for p in ["https://", "http://", "mailto:", "tel:", "sms:", "www."] {
        if s.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p)) {
            s = &s[p.len()..];
        }
    }
    if text.starts_with("BEGIN:VCARD") {
        if let Some(name) = text.lines().find_map(|l| l.strip_prefix("FN:").or_else(|| l.strip_prefix("FN;CHARSET=UTF-8:"))) {
            s = name.trim();
        }
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.chars().count() >= 40 {
            break;
        }
        if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    if out.is_empty() { format!("qr-{n}") } else { out }
}

#[derive(Clone)]
struct Style {
    modules: String,
    eyes: String,
    color: String,
    color2: String,
    fill: String,
    eye_color: Option<String>,
    background: Option<String>,
    margin: u32,
    ecc: Option<QrCodeEcc>,
    logo: Option<(String, f64)>,
    logo_size: f64,
    plate: bool,
    caption: String,
}

impl Style {
    fn from(req: &Request) -> Result<Self, FileError> {
        let color = |k: &str, d: &str| {
            let c = img::parse_color(req.str_or(k, d));
            format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
        };
        let logo_path = req.options.get("logo").and_then(|v| v.as_str().map(String::from).or_else(|| v["path"].as_str().map(String::from)));
        let logo = match logo_path.filter(|p| !p.is_empty()) {
            Some(p) => Some(logo_uri(Path::new(&p), req)?),
            None => None,
        };
        Ok(Style {
            modules: req.str_or("modules", "fluid").to_string(),
            eyes: req.str_or("eyes", "rounded").to_string(),
            color: color("color", "#1b1f2a"),
            color2: color("color2", "#d6338f"),
            fill: req.str_or("fill", "solid").to_string(),
            eye_color: req.bool_or("eyeColorOn", false).then(|| color("eyeColor", "#d6338f")),
            background: (req.str_or("background", "color") != "transparent").then(|| color("backgroundColor", "#ffffff")),
            margin: req.f64_or("margin", 4.0).clamp(0.0, 8.0) as u32,
            ecc: match req.str_or("ecc", "auto") {
                "L" => Some(QrCodeEcc::Low),
                "M" => Some(QrCodeEcc::Medium),
                "Q" => Some(QrCodeEcc::Quartile),
                "H" => Some(QrCodeEcc::High),
                _ => None,
            },
            logo,
            logo_size: req.f64_or("logoSize", 22.0).clamp(10.0, 30.0) / 100.0,
            plate: req.bool_or("logoPlate", true),
            caption: req.str_or("caption", "").trim().to_string(),
        })
    }
}

/// The logo as a data URI the SVG carries inside (so the SVG file stands alone), and its
/// width / height. Pictures are made at most 1024 px, as PNG to keep transparency.
fn logo_uri(path: &Path, req: &Request) -> Result<(String, f64), FileError> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "svg" {
        let data = std::fs::read(path).map_err(|_| FileError::new("bad_logo"))?;
        let opt = resvg::usvg::Options { fontdb: crate::common::font::system(), ..Default::default() };
        let tree = resvg::usvg::Tree::from_data(&data, &opt).map_err(|_| FileError::new("bad_logo"))?;
        // Normalized: no outside files or scripts left in it.
        let clean = tree.to_string(&resvg::usvg::WriteOptions::default());
        let s = tree.size();
        return Ok((format!("data:image/svg+xml;base64,{}", crate::common::b64::encode(clean.as_bytes())), (s.width() / s.height()) as f64));
    }
    let im = img::load(path, req).map_err(|_| FileError::new("bad_logo"))?.img;
    let im = if im.width().max(im.height()) > 1024 { im.resize(1024, 1024, image::imageops::FilterType::Lanczos3) } else { im };
    let png = img::encode_png(&im, false, 100)?;
    Ok((format!("data:image/png;base64,{}", crate::common::b64::encode(&png)), im.width() as f64 / im.height().max(1) as f64))
}

fn encode(text: &str, style: &Style) -> Result<QrCode, FileError> {
    if text.len() > MAX_BYTES {
        return Err(FileError::with("too_long", json!({ "bytes": text.len() })));
    }
    // A logo covers part of the code: the highest correction makes up for it.
    let ecc = style.ecc.unwrap_or(if style.logo.is_some() { QrCodeEcc::High } else { QrCodeEcc::Medium });
    let segs = QrSegment::make_segments(text);
    // A lower level when the text doesn't fit the one asked for, rather than no code.
    for e in [ecc, QrCodeEcc::Quartile, QrCodeEcc::Medium, QrCodeEcc::Low] {
        if (e as u8) > (ecc as u8) {
            continue;
        }
        if let Ok(q) = QrCode::encode_segments_advanced(&segs, e, qrcodegen::Version::MIN, qrcodegen::Version::MAX, None, true) {
            return Ok(q);
        }
    }
    Err(FileError::with("too_long", json!({ "bytes": text.len() })))
}

/// The code as SVG, in units of one dot.
fn draw(qr: &QrCode, st: &Style) -> String {
    let n = qr.size();
    let m = st.margin as f64;
    let w = n as f64 + 2.0 * m;
    let cap_h = if st.caption.is_empty() { 0.0 } else { (n as f64 * 0.14).max(3.2) };
    let h = w + cap_h;
    let dark = |x: i32, y: i32| x >= 0 && y >= 0 && x < n && y < n && qr.get_module(x, y);
    let in_eye = |x: i32, y: i32| (x < 7 && y < 7) || (x >= n - 7 && y < 7) || (x < 7 && y >= n - 7);

    // The middle left free for the logo, on whole dots.
    let hole = st.logo.as_ref().map(|(_, ratio)| {
        let side = n as f64 * st.logo_size;
        let (lw, lh) = if *ratio >= 1.0 { (side, side / ratio) } else { (side * ratio, side) };
        let pad = if st.plate { 0.8 } else { 0.0 };
        let c = n as f64 / 2.0;
        let (x0, x1) = ((c - lw / 2.0 - pad).floor(), (c + lw / 2.0 + pad).ceil());
        let (y0, y1) = ((c - lh / 2.0 - pad).floor(), (c + lh / 2.0 + pad).ceil());
        (x0, y0, x1, y1, lw, lh)
    });
    let in_hole = |x: i32, y: i32| match hole {
        Some((x0, y0, x1, y1, ..)) if st.plate => (x as f64) >= x0 && (x as f64) < x1 && (y as f64) >= y0 && (y as f64) < y1,
        _ => false,
    };

    let mut d = String::new();
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if !dark(x, y) || in_eye(x, y) || in_hole(x, y) {
                x += 1;
                continue;
            }
            match st.modules.as_str() {
                "dots" => {
                    let _ = write!(d, "M{} {}.5a.42 .42 0 1 0 .84 0a.42 .42 0 1 0 -.84 0z", x as f64 + 0.08, y);
                    x += 1;
                }
                "rounded" => {
                    rounded_rect(&mut d, x as f64 + 0.05, y as f64 + 0.05, 0.9, 0.9, [0.3; 4]);
                    x += 1;
                }
                "fluid" => {
                    let free = |dx: i32, dy: i32| !dark(x + dx, y + dy) || in_eye(x + dx, y + dy) || in_hole(x + dx, y + dy);
                    let r = |a: bool, b: bool| if a && b { 0.5 } else { 0.0 };
                    let (t, b, l, rt) = (free(0, -1), free(0, 1), free(-1, 0), free(1, 0));
                    rounded_rect(&mut d, x as f64, y as f64, 1.0, 1.0, [r(t, l), r(t, rt), r(b, rt), r(b, l)]);
                    x += 1;
                }
                _ => {
                    // A run of dark dots on the row, as one rectangle.
                    let start = x;
                    while x < n && dark(x, y) && !in_eye(x, y) && !in_hole(x, y) {
                        x += 1;
                    }
                    let _ = write!(d, "M{start} {y}h{}v1h-{}z", x - start, x - start);
                }
            }
        }
    }

    let mut eyes = String::new();
    let mut balls = String::new();
    for (ex, ey) in [(0.0, 0.0), ((n - 7) as f64, 0.0), (0.0, (n - 7) as f64)] {
        match st.eyes.as_str() {
            "circle" => {
                circle(&mut eyes, ex + 3.5, ey + 3.5, 3.5);
                circle(&mut eyes, ex + 3.5, ey + 3.5, 2.5);
                circle(&mut balls, ex + 3.5, ey + 3.5, 1.5);
            }
            "rounded" => {
                rounded_rect(&mut eyes, ex, ey, 7.0, 7.0, [2.0; 4]);
                rounded_rect(&mut eyes, ex + 1.0, ey + 1.0, 5.0, 5.0, [1.3; 4]);
                rounded_rect(&mut balls, ex + 2.0, ey + 2.0, 3.0, 3.0, [0.9; 4]);
            }
            _ => {
                let _ = write!(eyes, "M{ex} {ey}h7v7h-7zM{} {}h5v5h-5z", ex + 1.0, ey + 1.0);
                let _ = write!(balls, "M{} {}h3v3h-3z", ex + 2.0, ey + 2.0);
            }
        }
    }

    let mut s = String::new();
    let _ = write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{}" height="{}" shape-rendering="geometricPrecision">"#, w * 10.0, h * 10.0);
    let paint = if st.fill == "solid" {
        st.color.clone()
    } else {
        s.push_str("<defs>");
        if st.fill == "radial" {
            let _ = write!(s, r#"<radialGradient id="g" gradientUnits="userSpaceOnUse" cx="{c}" cy="{c}" r="{r}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></radialGradient>"#,
                           st.color, st.color2, c = n as f64 / 2.0, r = n as f64 * 0.72);
        } else {
            let _ = write!(s, r#"<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="{n}" y2="{n}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
                           st.color, st.color2);
        }
        s.push_str("</defs>");
        "url(#g)".to_string()
    };
    if let Some(bg) = &st.background {
        let _ = write!(s, r#"<rect width="{w}" height="{h}" fill="{bg}"/>"#);
    }
    let _ = write!(s, r#"<g transform="translate({m} {m})">"#);
    let _ = write!(s, r#"<path d="{d}" fill="{paint}"/>"#);
    let eye_paint = st.eye_color.clone().unwrap_or_else(|| paint.clone());
    let _ = write!(s, r#"<path d="{eyes}" fill="{eye_paint}" fill-rule="evenodd"/><path d="{balls}" fill="{eye_paint}"/>"#);
    if let (Some((uri, _)), Some((x0, y0, x1, y1, lw, lh))) = (&st.logo, hole) {
        if st.plate {
            let plate = st.background.clone().unwrap_or_else(|| "#ffffff".into());
            let r = ((x1 - x0).min(y1 - y0) * 0.18).min(2.0);
            let _ = write!(s, r#"<rect x="{x0}" y="{y0}" width="{}" height="{}" rx="{r}" fill="{plate}"/>"#, x1 - x0, y1 - y0);
        }
        let c = n as f64 / 2.0;
        let _ = write!(s, r#"<image x="{}" y="{}" width="{lw}" height="{lh}" preserveAspectRatio="xMidYMid meet" href="{uri}"/>"#, c - lw / 2.0, c - lh / 2.0);
    }
    s.push_str("</g>");
    if cap_h > 0.0 {
        let size = cap_h * 0.55;
        let y = m + n as f64 + (m * 0.35).min(1.2) + cap_h * 0.62;
        let ink = st.eye_color.clone().unwrap_or_else(|| st.color.clone());
        let _ = write!(s, r#"<text x="{}" y="{y}" font-family="Segoe UI, Arial, sans-serif" font-weight="600" font-size="{size}" text-anchor="middle" fill="{ink}">{}</text>"#,
                       w / 2.0, xml(&st.caption));
    }
    s.push_str("</svg>");
    s
}

fn rounded_rect(d: &mut String, x: f64, y: f64, w: f64, h: f64, r: [f64; 4]) {
    let [tl, tr, br, bl] = r;
    let _ = write!(d, "M{} {y}H{}", x + tl, x + w - tr);
    if tr > 0.0 {
        let _ = write!(d, "A{tr} {tr} 0 0 1 {} {}", x + w, y + tr);
    }
    let _ = write!(d, "V{}", y + h - br);
    if br > 0.0 {
        let _ = write!(d, "A{br} {br} 0 0 1 {} {}", x + w - br, y + h);
    }
    let _ = write!(d, "H{}", x + bl);
    if bl > 0.0 {
        let _ = write!(d, "A{bl} {bl} 0 0 1 {x} {}", y + h - bl);
    }
    let _ = write!(d, "V{}", y + tl);
    if tl > 0.0 {
        let _ = write!(d, "A{tl} {tl} 0 0 1 {} {y}", x + tl);
    }
    d.push('z');
}

fn circle(d: &mut String, cx: f64, cy: f64, r: f64) {
    let _ = write!(d, "M{} {cy}a{r} {r} 0 1 0 {} 0a{r} {r} 0 1 0 -{} 0z", cx - r, 2.0 * r, 2.0 * r);
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The SVG as PNG, `width` px wide. `opaque`: on white where it is transparent (JPG, and
/// the read-back check).
fn raster(svg: &str, width: u32, st: &Style, opaque: bool) -> Result<Vec<u8>, FileError> {
    let opt = resvg::usvg::Options { fontdb: crate::common::font::system(), ..Default::default() };
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(FileError::engine)?;
    let size = tree.size();
    let scale = width as f32 / size.width();
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or_else(|| FileError::new(codes::INVALID_OPTIONS))?;
    if opaque && st.background.is_none() {
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
    }
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(FileError::engine)
}

/// Whether the code reads back as `text`: from a picture the size a phone would see, on
/// white if transparent. The corner eyes are drawn square for this check only: the
/// reader used here finds just square ones, while phones read every shape offered;
/// what can stop a phone (the dots, the colors, the logo) is kept as it is.
fn scannable(qr: &QrCode, st: &Style, text: &str) -> Result<bool, FileError> {
    let mut plain = st.clone();
    plain.eyes = "square".into();
    plain.caption.clear();
    let png = raster(&draw(qr, &plain), (qr.size() as u32 + 2 * st.margin.max(2)) * 8, &plain, true)?;
    Ok(reads_back(&png, text))
}

/// Whether a reader finds the same text in the picture.
fn reads_back(png: &[u8], text: &str) -> bool {
    let Ok(im) = image::load_from_memory(png) else { return false };
    let mut prepared = rqrr::PreparedImage::prepare(im.to_luma8());
    prepared.detect_grids().iter().any(|g| g.decode().map(|(_, s)| s == text).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(modules: &str, eyes: &str, fill: &str) -> Style {
        Style {
            modules: modules.into(),
            eyes: eyes.into(),
            color: "#1b1f2a".into(),
            color2: "#d6338f".into(),
            fill: fill.into(),
            eye_color: None,
            background: Some("#ffffff".into()),
            margin: 4,
            ecc: None,
            logo: None,
            logo_size: 0.22,
            plate: true,
            caption: String::new(),
        }
    }

    #[test]
    fn every_style_reads_back() {
        let text = "https://mqkit.com/ro/unelte/coduri-qr?x=1";
        for m in ["square", "rounded", "fluid", "dots"] {
            for e in ["square", "rounded", "circle"] {
                for f in ["solid", "linear", "radial"] {
                    let st = style(m, e, f);
                    let qr = encode(text, &st).unwrap();
                    assert!(scannable(&qr, &st, text).unwrap(), "{m} {e} {f}");
                }
            }
        }
    }

    #[test]
    fn logo_and_caption_read_back() {
        let text = "WIFI:T:WPA;S:Casa;P:parola123;;";
        let mut st = style("fluid", "rounded", "linear");
        // A plain red square as the logo.
        let im = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 30, 30, 255])));
        st.logo = Some((format!("data:image/png;base64,{}", crate::common::b64::encode(&img::encode_png(&im, false, 100).unwrap())), 1.0));
        st.caption = "Scanează-mă & <hi>".into();
        for size in [0.10, 0.22, 0.30] {
            st.logo_size = size;
            let qr = encode(text, &st).unwrap();
            let svg = draw(&qr, &st);
            assert!(svg.contains("&amp; &lt;hi&gt;"));
            assert!(scannable(&qr, &st, text).unwrap(), "logo {size}");
        }
    }

    #[test]
    fn transparent_and_pale_codes() {
        let text = "hello";
        let mut st = style("dots", "circle", "solid");
        st.background = None;
        let qr = encode(text, &st).unwrap();
        assert!(scannable(&qr, &st, text).unwrap());
        // Pale yellow on white: a phone wouldn't read it, and neither does the check.
        st.color = "#f4f1c0".into();
        st.background = Some("#ffffff".into());
        assert!(!scannable(&qr, &st, text).unwrap());
    }

    #[test]
    fn contents_and_names() {
        let p = Path::new("a.txt");
        assert_eq!(contents("  one\n\ntwo \r\n", p, "lines"), vec!["one", "two"]);
        assert_eq!(contents("  one\ntwo ", p, "whole"), vec!["one\ntwo"]);
        assert_eq!(contents("\"a, b\",x\nc;d\n", Path::new("l.csv"), "whole"), vec!["a, b", "c"]);
        assert_eq!(contents("BEGIN:VCARD\nFN:Ana\nEND:VCARD\n", Path::new("c.vcf"), "lines"), vec!["BEGIN:VCARD\r\nFN:Ana\r\nEND:VCARD"]);
        assert_eq!(name_of("https://www.mqkit.com/ro/x?y=1", 1), "mqkit.com-ro-x-y-1");
        assert_eq!(name_of("BEGIN:VCARD\r\nFN:Ana Pop\r\nEND:VCARD", 1), "Ana-Pop");
        assert_eq!(name_of("???", 3), "qr-3");
        assert_eq!(name_of("東京タワー", 1), "東京タワー");
        assert_eq!(crate::common::b64::encode(b"Ma"), "TWE=");
        assert!(encode(&"x".repeat(3000), &style("square", "square", "solid")).is_err());
    }
}
