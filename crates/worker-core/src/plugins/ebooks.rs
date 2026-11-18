//! E-books: EPUB, Kindle (MOBI, AZW3 without DRM), FB2, Word/ODT, Markdown, HTML and
//! text into EPUB 3 (any reader; Kindle through Send to Kindle), PDF for an e-reader or
//! paper, Word or plain text; title, author and cover can be changed on the way.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::edge;
use contract::{opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const BOOKS: &[&str] = &["epub", "mobi", "azw3", "azw", "prc", "fb2", "docx", "odt", "rtf", "doc", "md", "markdown", "txt", "html", "htm"];
const OFFICE: &[&str] = &["docx", "odt", "rtf", "doc"];

#[derive(Default, Clone)]
struct Res {
    key: String,
    mime: String,
    data: Vec<u8>,
}

#[derive(Default, Clone)]
struct Chapter {
    title: String,
    /// The inside of <body>, as well-formed XHTML.
    body: String,
}

#[derive(Default)]
struct Book {
    title: String,
    authors: Vec<String>,
    language: String,
    cover: Option<String>,
    chapters: Vec<Chapter>,
    resources: Vec<Res>,
    css: String,
}

pub struct Ebooks;

impl Plugin for Ebooks {
    fn id(&self) -> &'static str {
        "ebooks"
    }

    fn manifest(&self) -> Value {
        let office = json!({ "file": "office", "eq": true, "quant": "any" });
        json!({
            "name": t("Cărți electronice", "E-books"),
            "summary": t("EPUB, Kindle, PDF", "EPUB, Kindle, PDF"),
            "keywords": { "ro": ["epub", "mobi", "azw3", "kindle", "carte electronică", "epub în pdf", "fb2"],
                          "en": ["epub", "mobi", "azw3", "kindle", "ebook", "epub to pdf", "fb2"] },
            "category": "document",
            "icon": "book",
            "requires": [ { "engine": "libreoffice", "when": { "any": [ office, { "field": "format", "eq": "docx" } ] } } ],
            "input": { "extensions": BOOKS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["epub", "pdf", "docx", "txt"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["title", "author", "office", "drm"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 2.0 },
            "presets": [],
            "groups": [ { "id": "meta", "label": t("Titlu, autor, copertă", "Title, author, cover"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "epub", "display": "chips",
                  "options": [ opt("epub", "EPUB (orice cititor, Kindle)", "EPUB (any reader, Kindle)"), opt("pdf", "PDF", "PDF"), opt("docx", "Word", "Word"), opt("txt", "Text", "Text") ] },
                { "key": "page", "type": "select", "label": t("Pagina", "Page"), "default": "ereader", "showIf": { "field": "format", "eq": "pdf" },
                  "options": [ opt("ereader", "Cititor de 6″ (Kindle, Kobo)", "6″ reader (Kindle, Kobo)"), opt("tablet", "Tabletă", "Tablet"), opt("a5", "A5", "A5"), opt("a4", "A4", "A4") ] },
                { "key": "fontSize", "type": "select", "label": t("Mărimea literelor", "Text size"), "default": "medium", "showIf": { "field": "format", "eq": "pdf" },
                  "options": [ opt("small", "Mică", "Small"), opt("medium", "Medie", "Medium"), opt("large", "Mare", "Large"), opt("xlarge", "Foarte mare", "Extra large") ] },
                { "key": "title", "type": "text", "default": "", "advanced": true, "group": "meta", "label": t("Titlu nou (opțional)", "New title (optional)") },
                { "key": "author", "type": "text", "default": "", "advanced": true, "group": "meta", "label": t("Autor nou (opțional)", "New author (optional)") },
                { "key": "cover", "type": "file", "default": "", "advanced": true, "group": "meta", "accept": ["jpg", "jpeg", "png"],
                  "label": t("Copertă nouă (opțional)", "New cover (optional)") }
            ],
            "rules": [
                { "id": "kindle", "severity": "info", "when": { "field": "format", "eq": "epub" },
                  "message": t("Pentru Kindle: trimite EPUB-ul cu Send to Kindle de la Amazon (din aplicație, de pe site sau pe e-mail).",
                               "For Kindle: send the EPUB with Amazon's Send to Kindle (app, website or e-mail).") },
                { "id": "drm", "severity": "error", "when": { "file": "drm", "eq": true, "quant": "any" },
                  "message": t("O carte are DRM (protecție la copiere) și nu poate fi convertită.", "A book has DRM (copy protection) and can't be converted.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "drm": t("Cartea are DRM (protecție la copiere) și nu poate fi convertită.", "The book has DRM (copy protection) and can't be converted."),
                "not_book": t("Fișierul nu este o carte pe care o putem citi.", "The file isn't a book we can read."),
                "mobi_compression": t("Această carte Kindle folosește o comprimare veche pe care nu o putem citi.", "This Kindle book uses an old compression we can't read."),
                "same_format": t("Fișierul e deja în acest format.", "The file is already in this format.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let path = f.input_path();
        let ext = ext_of(path);
        let mut p = Map::new();
        p.insert("office".into(), json!(OFFICE.contains(&ext.as_str())));
        p.insert("drm".into(), json!(false));
        match ext.as_str() {
            "epub" => {
                if let Ok(b) = read_epub(path) {
                    p.insert("title".into(), json!(b.title));
                    p.insert("author".into(), json!(b.authors.join(", ")));
                }
            }
            "mobi" | "azw3" | "azw" | "prc" => match std::fs::read(path).map_err(super::io_err).and_then(|d| read_mobi(&d)) {
                Ok(b) => {
                    p.insert("title".into(), json!(b.title));
                    p.insert("author".into(), json!(b.authors.join(", ")));
                }
                Err(e) if e.code == "drm" => {
                    p.insert("drm".into(), json!(true));
                }
                Err(_) => return Analysis::status("unsupported", "not_book"),
            },
            _ => {}
        }
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        let ext = ext_of(path);
        let format = req.str_or("format", "epub");
        // Same format: nothing to do, unless the title, author or cover changes.
        let changes = ["title", "author", "cover"].iter().any(|k| !req.str_or(k, "").trim().is_empty());
        if ext == format && !(changes && format == "epub") {
            return Err(FileError::skipped("same_format", json!({})));
        }
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let dir = ctx.file_dir(&f.id)?;
        let mut book = if OFFICE.contains(&ext.as_str()) {
            // Word and friends: LibreOffice makes the EPUB, then it is read like any other.
            let epub = super::word_to_pdf_lo::convert_with_lo(ctx, f, "epub", &ctx.work_dir.join(format!("lo-{}", f.id)))?;
            read_epub(&epub)?
        } else {
            read_any(path, &ext)?
        };
        apply_overrides(&mut book, req, path)?;
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.5, "write", 2, 2);
        let dest = dir.join(format!("result.{format}"));
        match format {
            "pdf" => write_pdf(ctx, &f.id, &book, &dest)?,
            "txt" => write_file(&dest, book_text(&book).as_bytes())?,
            "docx" => {
                let html = ctx.work_dir.join(format!("book-{}", f.id)).join("book.html");
                write_html_folder(&book, &html, req)?;
                // The pictures go inside the page: LibreOffice links the files of an HTML page
                // instead of copying them into the document, and those files are gone after.
                let page = std::fs::read_to_string(&html).map_err(super::io_err)?;
                let inlined = rewrite_refs(&page, &mut |v| {
                    let r = book.resources.iter().find(|r| r.key == v && r.mime.starts_with("image/"))?;
                    Some(format!("data:{};base64,{}", r.mime, super::markdown::base64(&r.data)))
                });
                write_file(&html, inlined.as_bytes())?;
                let mut fr = f.clone();
                fr.path = html.clone();
                fr.safe_path = None;
                let made = super::word_to_pdf_lo::convert_with_lo(ctx, &fr, "docx:MS Word 2007 XML", &ctx.work_dir.join(format!("docx-{}", f.id)))?;
                std::fs::rename(&made, &dest).or_else(|_| std::fs::copy(&made, &dest).map(|_| ())).map_err(super::io_err)?;
            }
            _ => write_file(&dest, &write_epub(&book)?)?,
        }
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        ctx.emit_output(&f.id, &dest, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn not_book() -> FileError {
    FileError::new("not_book")
}

fn mime_of(name: &str) -> String {
    let e = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match e.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "css" => "text/css",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
    .into()
}

fn ext_for_mime(m: &str) -> &'static str {
    match m {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "text/css" => "css",
        "font/ttf" | "application/x-font-ttf" | "application/font-sfnt" => "ttf",
        "font/otf" | "application/vnd.ms-opentype" => "otf",
        "font/woff" | "application/font-woff" => "woff",
        "font/woff2" => "woff2",
        _ => "bin",
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ------------------------------------------------------------------ HTML clean-up

const VOID: &[&str] = &["img", "br", "hr", "meta", "link", "input", "col", "area", "base", "wbr", "source", "param", "embed", "track"];
const BLOCK: &[&str] = &["p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "table", "blockquote", "pre", "section", "figure", "hr"];

/// HTML as found in books (Kindle files especially) into well-formed XHTML: void tags
/// closed, attributes quoted, stray "&" escaped, unclosed tags closed, Kindle-only tags
/// dropped. Not a full HTML parser, enough for book text.
fn xhtml(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut stack: Vec<String> = Vec::new();
    let b = html.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if html[i..].starts_with("<!--") {
                i = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(b.len());
                continue;
            }
            if html[i..].starts_with("<!") || html[i..].starts_with("<?") {
                i = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
                continue;
            }
            let Some(end) = tag_end(html, i) else {
                out.push_str("&lt;");
                i += 1;
                continue;
            };
            let raw = &html[i + 1..end];
            i = end + 1;
            let closing = raw.starts_with('/');
            let body = raw.trim_start_matches('/').trim_end_matches('/');
            let name: String = body.chars().take_while(|c| c.is_alphanumeric() || *c == ':' || *c == '-').collect::<String>().to_ascii_lowercase();
            if name.is_empty() || name.starts_with("mbp:") || matches!(name.as_str(), "html" | "head" | "body" | "title" | "script" | "style" | "meta" | "link" | "guide" | "reference") {
                if matches!(name.as_str(), "script" | "style" | "title") && !closing {
                    // Skip their content.
                    let close = format!("</{name}");
                    i = html[i..].to_ascii_lowercase().find(&close).map(|e| i + e).unwrap_or(b.len());
                    i = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
                }
                if name == "mbp:pagebreak" {
                    out.push_str("<div class=\"pagebreak\"></div>");
                }
                continue;
            }
            let name = if name == "font" { "span".to_string() } else { name };
            if closing {
                if let Some(pos) = stack.iter().rposition(|n| *n == name) {
                    while stack.len() > pos {
                        out.push_str(&format!("</{}>", stack.pop().unwrap()));
                    }
                }
                continue;
            }
            if BLOCK.contains(&name.as_str()) && stack.last().is_some_and(|n| n == "p") {
                out.push_str("</p>");
                stack.pop();
            }
            let attrs = clean_attrs(&body[body.find(|c: char| c.is_whitespace()).unwrap_or(body.len())..]);
            if VOID.contains(&name.as_str()) || raw.ends_with('/') {
                out.push_str(&format!("<{name}{attrs}/>"));
            } else {
                out.push_str(&format!("<{name}{attrs}>"));
                stack.push(name);
            }
        } else {
            let next = html[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            out.push_str(&fix_amp(&html[i..next]));
            i = next;
        }
    }
    while let Some(n) = stack.pop() {
        out.push_str(&format!("</{n}>"));
    }
    out
}

fn tag_end(s: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (k, c) in s[start + 1..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(start + 1 + k),
            (None, '<') => return None,
            _ => {}
        }
    }
    None
}

fn clean_attrs(s: &str) -> String {
    let mut out = String::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        while i < c.len() && (c[i].is_whitespace() || c[i] == '/') {
            i += 1;
        }
        let start = i;
        while i < c.len() && !c[i].is_whitespace() && c[i] != '=' && c[i] != '/' {
            i += 1;
        }
        let name: String = c[start..i].iter().collect::<String>().to_ascii_lowercase();
        if name.is_empty() {
            i += 1;
            continue;
        }
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        let mut value = name.clone();
        if i < c.len() && c[i] == '=' {
            i += 1;
            while i < c.len() && c[i].is_whitespace() {
                i += 1;
            }
            if i < c.len() && (c[i] == '"' || c[i] == '\'') {
                let q = c[i];
                i += 1;
                let vs = i;
                while i < c.len() && c[i] != q {
                    i += 1;
                }
                value = c[vs..i.min(c.len())].iter().collect();
                i += 1;
            } else {
                let vs = i;
                while i < c.len() && !c[i].is_whitespace() {
                    i += 1;
                }
                value = c[vs..i].iter().collect();
            }
        }
        // Kindle and script attributes are dropped.
        if name.starts_with("on") || matches!(name.as_str(), "filepos" | "recindex" | "aid" | "mediarecindex") || name.contains(':') && !name.starts_with("xml:") && !name.starts_with("epub:") && !name.starts_with("xlink:") {
            continue;
        }
        if out.contains(&format!(" {name}=\"")) {
            continue;
        }
        out.push_str(&format!(" {name}=\"{}\"", fix_amp(&value).replace('"', "&quot;").replace('<', "&lt;")));
    }
    out
}

/// "&" that doesn't start an entity becomes "&amp;"; HTML-only named entities become characters.
fn fix_amp(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let ent: String = rest[1..].chars().take_while(|c| c.is_alphanumeric() || *c == '#').collect();
        if rest[1..].starts_with(&format!("{ent};")) && !ent.is_empty() {
            match ent.as_str() {
                "amp" | "lt" | "gt" | "quot" | "apos" => out.push_str(&format!("&{ent};")),
                e if e.starts_with('#') => out.push_str(&format!("&{ent};")),
                "nbsp" => out.push('\u{a0}'),
                "mdash" => out.push('—'),
                "ndash" => out.push('–'),
                "hellip" => out.push('…'),
                "lsquo" => out.push('‘'),
                "rsquo" => out.push('’'),
                "ldquo" => out.push('“'),
                "rdquo" => out.push('”'),
                "copy" => out.push('©'),
                _ => out.push_str(&format!("&amp;{ent};")),
            }
            rest = &rest[ent.len() + 2..];
        } else {
            out.push_str("&amp;");
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Text of an XHTML fragment (for plain text and titles).
fn text_of(html: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let b = html.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let end = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
            let tag = html[i..end].to_ascii_lowercase();
            if ["<p", "<div", "<br", "<h1", "<h2", "<h3", "<h4", "<li", "<tr", "</p", "</div", "</h1", "</h2", "</h3"].iter().any(|t| tag.starts_with(t)) {
                out.push('\n');
            }
            i = end;
        } else {
            let next = html[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            out.push_str(&unescape(&html[i..next]));
            i = next;
        }
    }
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    let mut s = String::new();
    let mut blank = 0;
    for l in lines {
        if l.is_empty() {
            blank += 1;
            continue;
        }
        if !s.is_empty() {
            s.push_str(if blank > 0 { "\r\n\r\n" } else { "\r\n" });
        }
        s.push_str(l);
        blank = 0;
    }
    s
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let Some(e) = rest[p..].find(';').filter(|&e| e < 10) else {
            out.push('&');
            rest = &rest[p + 1..];
            continue;
        };
        let ent = &rest[p + 1..p + e];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&rest[p..p + e + 1]),
        }
        rest = &rest[p + e + 1..];
    }
    out.push_str(rest);
    out
}

/// One long text into chapters at its top headings (h1, else h2).
fn split_chapters(body: &str, fallback: &str) -> Vec<Chapter> {
    let lower = body.to_ascii_lowercase();
    let level = if lower.matches("<h1").count() >= 2 { "<h1" } else if lower.matches("<h2").count() >= 2 { "<h2" } else { "" };
    let mut cuts = vec![0];
    if !level.is_empty() {
        let mut at = 0;
        while let Some(p) = lower[at..].find(level) {
            let p = at + p;
            if p > 0 {
                cuts.push(p);
            }
            at = p + 3;
        }
    }
    cuts.push(body.len());
    cuts.dedup();
    cuts.windows(2).filter_map(|w| {
        let part = &body[w[0]..w[1]];
        if text_of(part).trim().is_empty() && !part.contains("<img") {
            return None;
        }
        let title = heading(part).unwrap_or_else(|| fallback.to_string());
        Some(Chapter { title, body: xhtml(part) })
    }).collect()
}

fn heading(part: &str) -> Option<String> {
    let lower = part.to_ascii_lowercase();
    for h in ["<h1", "<h2", "<h3"] {
        if let Some(s) = lower.find(h) {
            let close = format!("</{}", &h[1..]);
            let e = lower[s..].find(&close).map(|e| s + e)?;
            let t = text_of(&part[s..e]).replace("\r\n", " ");
            if !t.trim().is_empty() {
                return Some(t.trim().chars().take(120).collect());
            }
        }
    }
    None
}

// ------------------------------------------------------------------ reading

fn read_any(path: &Path, ext: &str) -> Result<Book, FileError> {
    match ext {
        "epub" => read_epub(path),
        "mobi" | "azw3" | "azw" | "prc" => read_mobi(&std::fs::read(path).map_err(super::io_err)?),
        "fb2" => read_fb2(&std::fs::read(path).map_err(super::io_err)?),
        "md" | "markdown" => {
            let text = crate::common::text::decode(&std::fs::read(path).map_err(super::io_err)?);
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new_ext(&text, pulldown_cmark::Options::all()));
            Ok(Book { title: heading(&html).unwrap_or_else(|| stem_of(path)), chapters: split_chapters(&html, &stem_of(path)), ..Default::default() })
        }
        "html" | "htm" => {
            let text = crate::common::text::decode(&std::fs::read(path).map_err(super::io_err)?);
            let lower = text.to_ascii_lowercase();
            let body = match (lower.find("<body"), lower.rfind("</body>")) {
                (Some(s), Some(e)) => &text[lower[s..].find('>').map(|x| s + x + 1).unwrap_or(s)..e],
                _ => &text,
            };
            Ok(Book { title: heading(body).unwrap_or_else(|| stem_of(path)), chapters: split_chapters(body, &stem_of(path)), ..Default::default() })
        }
        _ => {
            let text = crate::common::text::decode(&std::fs::read(path).map_err(super::io_err)?);
            let body: String = text.replace("\r\n", "\n").split("\n\n").map(str::trim).filter(|p| !p.is_empty()).map(|p| format!("<p>{}</p>", esc(p).replace('\n', "<br/>"))).collect();
            Ok(Book { title: stem_of(path), chapters: vec![Chapter { title: stem_of(path), body }], ..Default::default() })
        }
    }
}

/// Paths inside an EPUB, relative to a file.
fn join_path(base: &str, rel: &str) -> String {
    let rel = rel.split(['#', '?']).next().unwrap_or("");
    let rel = percent_decode(rel);
    let mut parts: Vec<&str> = if rel.starts_with('/') { Vec::new() } else { base.rsplit_once('/').map(|(d, _)| d.split('/').collect()).unwrap_or_default() };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every `attr="…"` value in `html`, replaced by `f` (attr: src, href, xlink:href; and url(…) in CSS).
fn rewrite_refs(html: &str, f: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let found = ["src=\"", "href=\"", "url(\"", "url('", "url("].iter().filter_map(|k| rest.find(k).map(|p| (p, *k))).min_by_key(|x| x.0);
        let Some((p, key)) = found else { break };
        let vs = p + key.len();
        let end_char = if key.ends_with('"') { '"' } else if key.ends_with('\'') { '\'' } else { ')' };
        let Some(ve) = rest[vs..].find(end_char).map(|e| vs + e) else { break };
        out.push_str(&rest[..vs]);
        let v = &rest[vs..ve];
        match f(v.trim().trim_matches(['"', '\''])) {
            Some(n) => out.push_str(&n),
            None => out.push_str(v),
        }
        rest = &rest[ve..];
    }
    out.push_str(rest);
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut at = 0;
    while let Some(p) = lower[at..].find(name) {
        let p = at + p;
        let before_ok = p == 0 || lower.as_bytes()[p - 1].is_ascii_whitespace();
        let after = &tag[p + name.len()..];
        let after_t = after.trim_start();
        if before_ok && after_t.starts_with('=') {
            let v = after_t[1..].trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                return v[1..].find(q).map(|e| unescape(&v[1..1 + e]));
            }
            return Some(v.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next()?.to_string());
        }
        at = p + name.len();
    }
    None
}

fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{name}");
    let mut at = 0;
    while let Some(p) = lower[at..].find(&open) {
        let p = at + p;
        let next = lower.as_bytes().get(p + open.len()).copied().unwrap_or(b' ');
        if next.is_ascii_whitespace() || next == b'>' || next == b'/' {
            let e = xml[p..].find('>').map(|e| p + e + 1).unwrap_or(xml.len());
            out.push(&xml[p..e]);
        }
        at = p + open.len();
    }
    out
}

/// The text inside the first `<name>…</name>` (namespace prefixes allowed).
fn element_text(xml: &str, name: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(p) = lower[at..].find(&format!(":{name}")).or_else(|| lower[at..].find(&format!("<{name}"))) {
        let p = at + p;
        let start = lower[..p + 1].rfind('<').unwrap_or(p);
        if lower[start..].starts_with("</") {
            at = p + 1;
            continue;
        }
        let Some(open_end) = xml[start..].find('>').map(|e| start + e + 1) else { break };
        let tag_name = &lower[start + 1..lower[start..].find(|c: char| c.is_whitespace() || c == '>').map(|e| start + e).unwrap_or(open_end)];
        let close = format!("</{tag_name}>");
        let Some(end) = lower[open_end..].find(&close).map(|e| open_end + e) else { at = open_end; continue };
        out.push(unescape(xml[open_end..end].trim()));
        at = end + close.len();
    }
    out
}

fn read_epub(path: &Path) -> Result<Book, FileError> {
    let file = std::fs::File::open(path).map_err(super::io_err)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| not_book())?;
    if zip.by_name("META-INF/encryption.xml").is_ok() {
        let mut s = String::new();
        let _ = zip.by_name("META-INF/encryption.xml").unwrap().take(16 << 20).read_to_string(&mut s);
        // Font obfuscation is fine; anything else is DRM.
        if s.contains("EncryptedData") && !s.contains("http://www.idpf.org/2008/embedding") && !s.contains("http://ns.adobe.com/pdf/enc#RC") {
            return Err(FileError::new("drm"));
        }
    }
    // Each part is unpacked with a cap, and the whole book too: a small file can unpack to
    // gigabytes (a zip bomb), which would stop the worker instead of failing this file.
    const PART_LIMIT: u64 = 256 << 20;
    const BOOK_LIMIT: u64 = 1536 << 20;
    let unpacked = std::cell::Cell::new(0u64);
    let too_big = std::cell::Cell::new(false);
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let e = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        e.take(PART_LIMIT + 1).read_to_end(&mut v).ok()?;
        unpacked.set(unpacked.get() + v.len() as u64);
        if v.len() as u64 > PART_LIMIT || unpacked.get() > BOOK_LIMIT {
            too_big.set(true);
            return None;
        }
        Some(v)
    };
    let large = || FileError::new(contract::codes::FILE_TOO_LARGE);
    let container = String::from_utf8_lossy(&read("META-INF/container.xml").ok_or_else(|| if too_big.get() { large() } else { not_book() })?).into_owned();
    let opf_path = tags(&container, "rootfile").first().and_then(|t| attr(t, "full-path")).ok_or_else(not_book)?;
    let opf = String::from_utf8_lossy(&read(&opf_path).ok_or_else(|| if too_big.get() { large() } else { not_book() })?).into_owned();
    let mut book = Book {
        title: element_text(&opf, "title").into_iter().next().unwrap_or_else(|| stem_of(path)),
        authors: element_text(&opf, "creator"),
        language: element_text(&opf, "language").into_iter().next().unwrap_or_default(),
        ..Default::default()
    };
    // The manifest: id → (path, type, properties).
    let mut items: HashMap<String, (String, String, String)> = HashMap::new();
    for it in tags(&opf, "item") {
        if let (Some(id), Some(href)) = (attr(it, "id"), attr(it, "href")) {
            items.insert(id, (join_path(&opf_path, &href), attr(it, "media-type").unwrap_or_default(), attr(it, "properties").unwrap_or_default()));
        }
    }
    let cover_id = tags(&opf, "meta").iter().find(|m| attr(m, "name").as_deref() == Some("cover")).and_then(|m| attr(m, "content"));
    let cover_path = items.iter().find(|(id, (_, mt, props))| (props.contains("cover-image") || cover_id.as_deref() == Some(id.as_str())) && mt.starts_with("image/")).map(|(_, (p, _, _))| p.clone());
    // Resources get short names; chapters keep their order from the spine.
    let mut keys: HashMap<String, String> = HashMap::new();
    let mut add_res = |book: &mut Book, p: &str, read: &mut dyn FnMut(&str) -> Option<Vec<u8>>| -> Option<String> {
        if let Some(k) = keys.get(p) {
            return Some(k.clone());
        }
        let data = read(p)?;
        let mime = mime_of(p);
        let key = format!("r{}.{}", book.resources.len() + 1, ext_for_mime(&mime));
        keys.insert(p.to_string(), key.clone());
        book.resources.push(Res { key: key.clone(), mime, data });
        Some(key)
    };
    if let Some(cp) = &cover_path {
        book.cover = add_res(&mut book, cp, &mut read);
    }
    let spine: Vec<String> = tags(&opf, "itemref").iter().filter_map(|t| attr(t, "idref")).collect();
    let mut css_done = std::collections::HashSet::new();
    for id in spine {
        if too_big.get() {
            return Err(large());
        }
        let Some((p, mt, _)) = items.get(&id).cloned() else { continue };
        if !mt.contains("html") {
            continue;
        }
        let Some(bytes) = read(&p) else { continue };
        let html = String::from_utf8_lossy(&bytes).into_owned();
        // The book's styles, once each.
        for l in tags(&html, "link") {
            if attr(l, "rel").is_some_and(|r| r.contains("stylesheet")) {
                if let Some(href) = attr(l, "href") {
                    let cp = join_path(&p, &href);
                    if css_done.insert(cp.clone()) {
                        if let Some(css) = read(&cp) {
                            let css = String::from_utf8_lossy(&css).into_owned();
                            let css = rewrite_refs(&css, &mut |r| add_res(&mut book, &join_path(&cp, r), &mut read));
                            book.css.push_str(&css);
                            book.css.push('\n');
                        }
                    }
                }
            }
        }
        let lower = html.to_ascii_lowercase();
        let body = match (lower.find("<body"), lower.rfind("</body>")) {
            (Some(s), Some(e)) => &html[lower[s..].find('>').map(|x| s + x + 1).unwrap_or(s)..e],
            _ => &html[..],
        };
        let body = rewrite_refs(body, &mut |r| {
            if r.starts_with('#') || r.contains("://") || r.starts_with("mailto:") {
                return None;
            }
            let target = join_path(&p, r);
            if target.ends_with("html") || target.ends_with(".htm") {
                return None;
            }
            add_res(&mut book, &target, &mut read)
        });
        let title = heading(&body).or_else(|| element_text(&html, "title").into_iter().next()).unwrap_or_default();
        book.chapters.push(Chapter { title, body: xhtml(&body) });
    }
    if too_big.get() {
        return Err(large());
    }
    if book.chapters.is_empty() {
        return Err(not_book());
    }
    Ok(book)
}

// --- Kindle: MOBI (PalmDOC) and KF8 (AZW3)

fn be16(d: &[u8], at: usize) -> usize {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize).unwrap_or(0)
}

fn be32(d: &[u8], at: usize) -> usize {
    d.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize).unwrap_or(0)
}

/// PalmDOC's LZ77.
fn palmdoc(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 2);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        match c {
            0x01..=0x08 => {
                let n = c as usize;
                out.extend_from_slice(&src[i..(i + n).min(src.len())]);
                i += n;
            }
            0x00 | 0x09..=0x7F => out.push(c),
            0x80..=0xBF => {
                let Some(&c2) = src.get(i) else { break };
                i += 1;
                let pair = ((c as usize) << 8 | c2 as usize) & 0x3FFF;
                let dist = pair >> 3;
                let len = (pair & 7) + 3;
                if dist == 0 || dist > out.len() {
                    continue;
                }
                for _ in 0..len {
                    let b = out[out.len() - dist];
                    out.push(b);
                }
            }
            _ => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
    out
}

/// A text record without its trailing entries (sizes at the end, flagged in the header).
fn trim_trailing(rec: &[u8], flags: usize) -> &[u8] {
    let mut end = rec.len();
    for bit in 1..16 {
        if flags & (1 << bit) != 0 {
            // Backward-encoded size: the high bit marks its first byte.
            let mut size = 0usize;
            let mut shift = 0;
            let mut pos = end;
            while pos > 0 {
                pos -= 1;
                let b = rec[pos];
                size |= ((b & 0x7F) as usize) << shift;
                shift += 7;
                if b & 0x80 != 0 || shift > 28 {
                    break;
                }
            }
            end = end.saturating_sub(size);
        }
    }
    if flags & 1 != 0 && end > 0 {
        let n = (rec[end - 1] & 3) as usize + 1;
        end = end.saturating_sub(n);
    }
    &rec[..end]
}

fn read_mobi(d: &[u8]) -> Result<Book, FileError> {
    if d.get(60..68) != Some(b"BOOKMOBI") && d.get(60..68) != Some(b"TEXtREAd") {
        return Err(not_book());
    }
    let count = be16(d, 76);
    let offsets: Vec<usize> = (0..count).map(|i| be32(d, 78 + i * 8)).collect();
    let record = |i: usize| -> &[u8] {
        let s = *offsets.get(i).unwrap_or(&d.len());
        let e = offsets.get(i + 1).copied().unwrap_or(d.len());
        d.get(s..e.max(s)).unwrap_or(&[])
    };
    let mut r0 = record(0);
    let mut base = 0usize;
    // A joint file (MOBI 6 + KF8): the KF8 part starts at the record EXTH 121 gives.
    let exth = |r0: &[u8]| -> HashMap<u32, Vec<Vec<u8>>> {
        let mut m: HashMap<u32, Vec<Vec<u8>>> = HashMap::new();
        let hl = be32(r0, 20);
        if be32(r0, 128) & 0x40 != 0 && r0.get(16 + hl..16 + hl + 4) == Some(b"EXTH") {
            let at = 16 + hl;
            let n = be32(r0, at + 8);
            let mut p = at + 12;
            for _ in 0..n {
                let (kind, len) = (be32(r0, p) as u32, be32(r0, p + 4));
                if len < 8 {
                    break;
                }
                m.entry(kind).or_default().push(r0.get(p + 8..p + len).unwrap_or(&[]).to_vec());
                p += len;
            }
        }
        m
    };
    let mut ex = exth(r0);
    if let Some(b) = ex.get(&121).and_then(|v| v.first()) {
        let k = u32::from_be_bytes(b.get(..4).and_then(|x| x.try_into().ok()).unwrap_or([0xFF; 4])) as usize;
        if k > 0 && k < count && record(k).get(16..20) == Some(b"MOBI") {
            base = k;
            r0 = record(k);
            ex = exth(r0);
        }
    }
    let compression = be16(r0, 0);
    let text_records = be16(r0, 8);
    if be16(r0, 12) != 0 {
        return Err(FileError::new("drm"));
    }
    if compression == 17480 {
        return Err(FileError::new("mobi_compression"));
    }
    let header_len = be32(r0, 20);
    let encoding = be32(r0, 28);
    let version = be32(r0, 36);
    let first_image = be32(r0, 108);
    let extra_flags = if header_len >= 0xE4 { be16(r0, 16 + 0xE2) } else { 0 };
    let mut raw = Vec::new();
    for i in 1..=text_records {
        let rec = trim_trailing(record(base + i), extra_flags);
        if compression == 2 {
            raw.extend(palmdoc(rec));
        } else {
            raw.extend_from_slice(rec);
        }
    }
    // KF8: only the main text flow (the others are styles and pictures in SVG).
    if version >= 8 {
        let fdst = be32(r0, 16 + 0xC0);
        let f = record(base + fdst);
        if fdst > 0 && f.get(..4) == Some(b"FDST") {
            let start = be32(f, 12);
            let end = be32(f, 16);
            let n = be32(f, 8);
            let mut css = String::new();
            for k in 1..n {
                let (s, e) = (be32(f, 12 + k * 8), be32(f, 16 + k * 8));
                let flow = String::from_utf8_lossy(raw.get(s..e.min(raw.len())).unwrap_or(&[])).into_owned();
                if flow.contains('{') && !flow.trim_start().starts_with('<') {
                    css.push_str(&flow);
                }
            }
            raw = raw.get(start..end.min(raw.len())).unwrap_or(&raw).to_vec();
            if !css.is_empty() {
                raw.splice(0..0, format!("<style>{css}</style>").into_bytes());
            }
        }
    }
    let html = if encoding == 1252 { encoding_rs::WINDOWS_1252.decode(&raw).0.into_owned() } else { String::from_utf8_lossy(&raw).into_owned() };
    // Kindle styles go to the book's CSS; pictures become resources.
    let mut book = Book::default();
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(s) = lower[at..].find("<style") {
        let s = at + s;
        let open_end = lower[s..].find('>').map(|e| s + e + 1).unwrap_or(lower.len());
        let e = lower[open_end..].find("</style>").map(|e| open_end + e).unwrap_or(lower.len());
        book.css.push_str(&html[open_end..e]);
        at = e;
    }
    let mut keys: HashMap<usize, String> = HashMap::new();
    let mut image = |book: &mut Book, n: usize| -> Option<String> {
        if n == 0 {
            return None;
        }
        if let Some(k) = keys.get(&n) {
            return Some(k.clone());
        }
        let data = record(first_image + n - 1);
        let mime = match data.get(..4)? {
            [0xFF, 0xD8, ..] => "image/jpeg",
            [0x89, b'P', b'N', b'G'] => "image/png",
            [b'G', b'I', b'F', _] => "image/gif",
            _ => return None,
        };
        let key = format!("r{}.{}", book.resources.len() + 1, ext_for_mime(mime));
        book.resources.push(Res { key: key.clone(), mime: mime.into(), data: data.to_vec() });
        keys.insert(n, key.clone());
        Some(key)
    };
    // MOBI 6: <img recindex="00012">; KF8: src="kindle:embed:000C?mime=…" (base 32).
    let mut body = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(p) = rest.find("<img") {
        body.push_str(&rest[..p]);
        let e = rest[p..].find('>').map(|e| p + e + 1).unwrap_or(rest.len());
        let tag = &rest[p..e];
        let n = attr(tag, "recindex").and_then(|v| v.parse::<usize>().ok()).or_else(|| {
            attr(tag, "src").and_then(|s| s.strip_prefix("kindle:embed:").map(|x| x.split('?').next().unwrap_or("").to_string())).and_then(|b32| usize::from_str_radix(&b32, 32).ok())
        });
        match n.and_then(|n| image(&mut book, n)) {
            Some(k) => body.push_str(&format!("<img src=\"{k}\" alt=\"\"/>")),
            None => body.push_str(tag),
        }
        rest = &rest[e..];
    }
    body.push_str(rest);
    let exth_text = |k: u32| ex.get(&k).and_then(|v| v.first()).map(|b| String::from_utf8_lossy(b).trim().to_string()).filter(|s| !s.is_empty());
    let full_name = {
        let (o, l) = (be32(r0, 84), be32(r0, 88));
        String::from_utf8_lossy(r0.get(o..o + l).unwrap_or(&[])).trim().to_string()
    };
    book.title = exth_text(503).unwrap_or(full_name);
    book.authors = ex.get(&100).map(|v| v.iter().map(|b| String::from_utf8_lossy(b).trim().to_string()).collect()).unwrap_or_default();
    book.language = exth_text(524).unwrap_or_default();
    if let Some(off) = ex.get(&201).and_then(|v| v.first()).and_then(|b| b.get(..4)).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize) {
        book.cover = image(&mut book, off + 1);
    }
    // Chapters at Kindle page breaks, else at headings.
    let title = book.title.clone();
    let parts: Vec<&str> = body.split("<mbp:pagebreak").collect();
    if parts.len() > 2 {
        for part in parts {
            let part = part.find('>').map(|e| &part[e + 1..]).filter(|_| part.trim_start().starts_with('/') || part.starts_with(' ') || part.starts_with('>')).unwrap_or(part);
            book.chapters.extend(split_chapters(part, &title));
        }
    } else {
        book.chapters = split_chapters(&body, &title);
    }
    if book.chapters.is_empty() {
        return Err(not_book());
    }
    Ok(book)
}

// --- FictionBook 2

fn read_fb2(d: &[u8]) -> Result<Book, FileError> {
    let xml = crate::common::text::decode(d);
    let info = xml.find("<title-info").and_then(|s| xml[s..].find("</title-info>").map(|e| &xml[s..s + e])).unwrap_or("");
    let mut book = Book {
        title: element_text(info, "book-title").into_iter().next().unwrap_or_default(),
        language: element_text(info, "lang").into_iter().next().unwrap_or_default(),
        ..Default::default()
    };
    for a in element_text(info, "author") {
        let name: Vec<String> = ["first-name", "middle-name", "last-name"].iter().flat_map(|k| element_text(&format!("<x>{a}</x>"), k)).collect();
        if !name.is_empty() {
            book.authors.push(name.join(" "));
        }
    }
    // Pictures: <binary id="…" content-type="…">base64</binary>
    let mut ids: HashMap<String, String> = HashMap::new();
    for (i, t) in tags(&xml, "binary").into_iter().enumerate() {
        let (Some(id), mime) = (attr(t, "id"), attr(t, "content-type").unwrap_or_else(|| "image/jpeg".into())) else { continue };
        let s = xml.find(t).map(|p| p + t.len()).unwrap_or(0);
        let e = xml[s..].find("</binary>").map(|e| s + e).unwrap_or(s);
        let data = base64_decode(&xml[s..e]);
        let key = format!("r{}.{}", i + 1, ext_for_mime(&mime));
        book.resources.push(Res { key: key.clone(), mime, data });
        ids.insert(id, key);
    }
    let cover_ref = info.find("<coverpage").and_then(|s| tags(&info[s..], "image").first().and_then(|t| attr(t, "l:href").or_else(|| attr(t, "xlink:href"))));
    book.cover = cover_ref.and_then(|r| ids.get(r.trim_start_matches('#')).cloned());
    // Body: FB2 tags into XHTML.
    let body_start = xml.find("<body").ok_or_else(not_book)?;
    let body_end = xml.rfind("</body>").unwrap_or(xml.len());
    let mut body = xml[body_start..body_end].to_string();
    for (from, to) in [("<emphasis", "<em"), ("</emphasis>", "</em>"), ("<strikethrough", "<s"), ("</strikethrough>", "</s>"), ("<empty-line/>", "<br/>"),
                       ("<subtitle", "<h3"), ("</subtitle>", "</h3>"), ("<v>", "<p class=\"verse\">"), ("</v>", "</p>"), ("<text-author", "<p class=\"author\""), ("</text-author>", "</p>")] {
        body = body.replace(from, to);
    }
    // Titles of sections become headings; sections become divs.
    body = body.replace("<title>", "<h2>").replace("</title>", "</h2>").replace("<section", "<div").replace("</section>", "</div>")
        .replace("<epigraph", "<blockquote").replace("</epigraph>", "</blockquote>").replace("<poem", "<div").replace("</poem>", "</div>")
        .replace("<stanza", "<div").replace("</stanza>", "</div>").replace("<cite", "<blockquote").replace("</cite>", "</blockquote>");
    let body = rewrite_refs(&body, &mut |r| ids.get(r.trim_start_matches('#')).cloned());
    let body = body.replace("l:href=", "src=").replace("xlink:href=", "src=").replace("<image", "<img");
    let first = body.find('>').map(|e| e + 1).unwrap_or(0);
    let title = book.title.clone();
    book.chapters = split_chapters(&body[first..], &title);
    if book.chapters.is_empty() {
        return Err(not_book());
    }
    Ok(book)
}

fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

fn apply_overrides(book: &mut Book, req: &Request, path: &Path) -> Result<(), FileError> {
    let title = req.str_or("title", "").trim();
    if !title.is_empty() {
        book.title = title.to_string();
    }
    let author = req.str_or("author", "").trim();
    if !author.is_empty() {
        book.authors = vec![author.to_string()];
    }
    if book.title.trim().is_empty() {
        book.title = stem_of(path);
    }
    let cover = req.str_or("cover", "").trim();
    if !cover.is_empty() {
        let data = std::fs::read(cover).map_err(super::io_err)?;
        let mime = mime_of(cover);
        let key = format!("r{}.{}", book.resources.len() + 1, ext_for_mime(&mime));
        book.resources.push(Res { key: key.clone(), mime, data });
        book.cover = Some(key);
    }
    Ok(())
}

// ------------------------------------------------------------------ writing

const BASE_CSS: &str = "body{margin:0 4%;line-height:1.45}img{max-width:100%;height:auto}.pagebreak{break-after:page}p.verse{margin:0 0 0 2em}";

fn chapter_doc(title: &str, body: &str, lang: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{lang}\" lang=\"{lang}\"><head><meta charset=\"utf-8\"/><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head><body>{body}</body></html>",
        esc(title)
    )
}

fn write_epub(b: &Book) -> Result<Vec<u8>, FileError> {
    let err = |e: zip::result::ZipError| FileError::engine(e);
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("mimetype", stored).map_err(err)?;
    z.write_all(b"application/epub+zip").map_err(super::io_err)?;
    z.start_file("META-INF/container.xml", deflated).map_err(err)?;
    z.write_all(br#"<?xml version="1.0" encoding="utf-8"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).map_err(super::io_err)?;
    let lang = if b.language.is_empty() { "und".to_string() } else { esc(&b.language) };
    let uid = format!("urn:uuid:{:x}", {
        let mut h: u64 = 0xcbf29ce484222325;
        for c in b.title.bytes().chain(b.authors.join(",").bytes()) {
            h = (h ^ c as u64).wrapping_mul(0x100000001b3);
        }
        h
    });
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav = String::new();
    let mut ncx = String::new();
    if let Some(cover) = &b.cover {
        let doc = chapter_doc(&b.title, &format!("<div style=\"text-align:center\"><img src=\"{cover}\" alt=\"{}\" style=\"max-height:100%\"/></div>", esc(&b.title)), &lang);
        z.start_file("OEBPS/cover.xhtml", deflated).map_err(err)?;
        z.write_all(doc.as_bytes()).map_err(super::io_err)?;
        manifest.push_str("<item id=\"cover-page\" href=\"cover.xhtml\" media-type=\"application/xhtml+xml\"/>");
        spine.push_str("<itemref idref=\"cover-page\"/>");
    }
    for (i, c) in b.chapters.iter().enumerate() {
        let name = format!("c{}.xhtml", i + 1);
        z.start_file(format!("OEBPS/{name}"), deflated).map_err(err)?;
        z.write_all(chapter_doc(&c.title, &c.body, &lang).as_bytes()).map_err(super::io_err)?;
        manifest.push_str(&format!("<item id=\"c{}\" href=\"{name}\" media-type=\"application/xhtml+xml\"/>", i + 1));
        spine.push_str(&format!("<itemref idref=\"c{}\"/>", i + 1));
        let title = if c.title.is_empty() { format!("{}", i + 1) } else { esc(&c.title) };
        nav.push_str(&format!("<li><a href=\"{name}\">{title}</a></li>"));
        ncx.push_str(&format!("<navPoint id=\"n{0}\" playOrder=\"{0}\"><navLabel><text>{title}</text></navLabel><content src=\"{name}\"/></navPoint>", i + 1));
    }
    for r in &b.resources {
        z.start_file(format!("OEBPS/{}", r.key), deflated).map_err(err)?;
        z.write_all(&r.data).map_err(super::io_err)?;
        let props = if b.cover.as_deref() == Some(&r.key) { " properties=\"cover-image\"" } else { "" };
        manifest.push_str(&format!("<item id=\"{}\" href=\"{}\" media-type=\"{}\"{props}/>", r.key.replace('.', "_"), r.key, r.mime));
    }
    z.start_file("OEBPS/style.css", deflated).map_err(err)?;
    z.write_all(format!("{BASE_CSS}\n{}", b.css).as_bytes()).map_err(super::io_err)?;
    z.start_file("OEBPS/nav.xhtml", deflated).map_err(err)?;
    z.write_all(chapter_doc(&b.title, &format!("<nav epub:type=\"toc\" id=\"toc\"><h1>{}</h1><ol>{nav}</ol></nav>", esc(&b.title)), &lang).as_bytes()).map_err(super::io_err)?;
    z.start_file("OEBPS/toc.ncx", deflated).map_err(err)?;
    z.write_all(format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><head><meta name=\"dtb:uid\" content=\"{uid}\"/></head><docTitle><text>{}</text></docTitle><navMap>{ncx}</navMap></ncx>", esc(&b.title)).as_bytes()).map_err(super::io_err)?;
    let creators: String = b.authors.iter().map(|a| format!("<dc:creator>{}</dc:creator>", esc(a))).collect();
    let cover_meta = b.cover.as_ref().map(|c| format!("<meta name=\"cover\" content=\"{}\"/>", c.replace('.', "_"))).unwrap_or_default();
    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"uid\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"uid\">{uid}</dc:identifier><dc:title>{}</dc:title>{creators}<dc:language>{lang}</dc:language><meta property=\"dcterms:modified\">{modified}</meta>{cover_meta}</metadata>\
         <manifest><item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/><item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>{manifest}</manifest><spine toc=\"ncx\">{spine}</spine></package>",
        esc(&b.title)
    );
    z.start_file("OEBPS/content.opf", deflated).map_err(err)?;
    z.write_all(opf.as_bytes()).map_err(super::io_err)?;
    Ok(z.finish().map_err(err)?.into_inner())
}

fn book_text(b: &Book) -> String {
    let mut s = format!("{}\r\n", b.title);
    if !b.authors.is_empty() {
        s.push_str(&format!("{}\r\n", b.authors.join(", ")));
    }
    for c in &b.chapters {
        // Soft hyphens help line breaking on screens; in plain text they only get in the way.
        let t = text_of(&c.body).replace('\u{ad}', "");
        if !t.trim().is_empty() {
            s.push_str("\r\n\r\n");
            s.push_str(t.trim());
        }
    }
    s.push_str("\r\n");
    s
}

/// The whole book as one HTML page with its pictures in the same folder.
fn write_html_folder(b: &Book, html: &Path, req: &Request) -> Result<(), FileError> {
    let dir = html.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(super::io_err)?;
    for r in &b.resources {
        write_file(&dir.join(&r.key), &r.data)?;
    }
    let (size, margin) = match req.str_or("page", "ereader") {
        "a4" => ("A4", "20mm"),
        "a5" => ("A5", "15mm"),
        "tablet" => ("150mm 200mm", "10mm"),
        _ => ("90mm 122mm", "6mm"),
    };
    let font = match req.str_or("fontSize", "medium") {
        "small" => "9.5pt",
        "large" => "13pt",
        "xlarge" => "15pt",
        _ => "11pt",
    };
    let mut body = String::new();
    if let Some(c) = &b.cover {
        body.push_str(&format!("<section class=\"cover\"><img src=\"{c}\" alt=\"\"/></section>"));
    }
    body.push_str(&format!("<section class=\"titlepage\"><h1>{}</h1><p>{}</p></section>", esc(&b.title), esc(&b.authors.join(", "))));
    for c in &b.chapters {
        body.push_str(&format!("<section class=\"chapter\">{}</section>", c.body));
    }
    let page = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src 'self' file: data:; style-src 'unsafe-inline'; font-src 'self' file: data:\"><title>{}</title>\
         <style>{}\n@page{{size:{size};margin:{margin}}} html,body{{font-size:{font}}} body{{margin:0;font-family:Georgia,'Times New Roman',serif}}\
         section.chapter,section.titlepage{{break-before:page}} section.cover{{height:100vh;display:flex;align-items:center;justify-content:center}} section.cover img{{max-height:100%;max-width:100%}}\
         section.titlepage{{text-align:center;padding-top:30%}} {BASE_CSS}</style></head><body>{body}</body></html>",
        esc(&b.title), b.css
    );
    write_file(html, page.as_bytes())
}

fn write_pdf(ctx: &Ctx, id: &str, b: &Book, dest: &Path) -> Result<(), FileError> {
    let html = ctx.work_dir.join(format!("book-{id}")).join("book.html");
    write_html_folder(b, &html, ctx.req)?;
    edge::print_with(ctx, id, &html, dest, Duration::from_secs(600), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy() {
        assert_eq!(xhtml("<p>A &amp B<br><img src=x.jpg><p>C"), "<p>A &amp;amp B<br/><img src=\"x.jpg\"/></p><p>C</p>");
        assert_eq!(xhtml("<font size=3 color='red'>x</font><mbp:pagebreak/>y"), "<span size=\"3\" color=\"red\">x</span><div class=\"pagebreak\"></div>y");
        assert_eq!(xhtml("<div onclick=\"x()\">&nbsp;&mdash;</div>"), "<div>\u{a0}—</div>");
    }

    #[test]
    fn palmdoc_decode() {
        // "abcabcabc": literal "abc" then a back-reference of 6 at distance 3.
        let packed = [b'a', b'b', b'c', 0x80, 0x1B];
        assert_eq!(palmdoc(&packed), b"abcabcabc");
    }

    #[test]
    fn paths() {
        assert_eq!(join_path("OEBPS/text/ch1.xhtml", "../images/a%20b.jpg#x"), "OEBPS/images/a b.jpg");
        assert_eq!(join_path("content.opf", "text/c.xhtml"), "text/c.xhtml");
    }
}
