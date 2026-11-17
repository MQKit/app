//! Markdown and text to a good-looking PDF or a self-contained web page, with a
//! choice of themes and an optional table of contents.

use super::html_to_pdf::{page_css, page_fields};
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::edge;
use contract::{opt, t, FileError, FileRef, Request};
use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{json, Map, Value};
use std::time::Duration;

pub struct Markdown;

impl Plugin for Markdown {
    fn id(&self) -> &'static str {
        "markdown"
    }

    fn manifest(&self) -> Value {
        let mut fields = vec![
            json!({ "key": "format", "type": "select", "label": t("Rezultat", "Result"), "default": "pdf", "display": "chips",
                    "options": [ opt("pdf", "PDF", "PDF"), opt("html", "Pagină web (HTML)", "Web page (HTML)") ] }),
            json!({ "key": "theme", "type": "select", "label": t("Aspect", "Look"), "default": "clean",
                    "options": [ opt("clean", "Simplu și clar", "Clean"), opt("book", "Carte (cu serife)", "Book (serif)"),
                                 opt("compact", "Compact", "Compact") ] }),
            json!({ "key": "toc", "type": "toggle", "default": false, "label": t("Cuprins la început", "Table of contents at the start") }),
        ];
        for mut f in page_fields() {
            f["showIf"] = json!({ "field": "format", "eq": "pdf" });
            fields.push(f);
        }
        json!({
            "name": t("Markdown și text", "Markdown and text"),
            "summary": t("În PDF sau HTML", "To PDF or HTML"),
            "keywords": { "ro": ["markdown", "md", "text", "txt", "notițe", "readme"], "en": ["markdown", "md", "text", "txt", "notes", "readme"] },
            "category": "document",
            "icon": "code",
            "input": { "extensions": ["md", "markdown", "txt", "text"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf", "html"], "replaceOriginalAllowed": false },
            "analysis": { "fast": [] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [],
            "groups": [],
            "fields": fields,
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {}
        })
    }

    fn analyze(&self, _f: &FileRef, _req: &Request) -> Analysis {
        Analysis::ok(Map::new())
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "render", 1, 1);
        let raw = std::fs::read(path).map_err(super::io_err)?;
        let text = decode(&raw);
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let format = req.str_or("format", "pdf");
        let src_dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let (text, meta_title) = if matches!(ext.as_str(), "txt" | "text") { (text, None) } else { front_matter(&text) };
        let title = meta_title.unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        // A web page carries its pictures inside, so it still shows them when moved.
        let inline_from = (format == "html").then_some(src_dir.as_path());
        let body = if matches!(ext.as_str(), "txt" | "text") { text_to_html(&text) } else { md_to_html(&text, req.bool_or("toc", false), inline_from) };
        let mut css = theme(req.str_or("theme", "clean")).to_string();
        if format == "pdf" {
            css.push_str(&page_css(req));
            // On paper nothing scrolls: long code lines wrap, and blocks stay whole.
            css.push_str("pre{white-space:pre-wrap;overflow-wrap:anywhere;overflow:visible}pre,table,img,blockquote{break-inside:avoid}h1,h2,h3,h4{break-after:avoid}");
        }
        // The language of the page (hyphenation, fonts): the one the app speaks.
        let lang = req.locale.as_deref().and_then(|l| l.split(['-', '_']).next()).filter(|l| !l.is_empty()).unwrap_or("en");
        // Markdown may hold raw HTML: its scripts never run.
        let csp = "<meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none'; object-src 'none'; frame-src 'none'\">";
        let page = format!(
            "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">{csp}<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>{css}</style></head><body><main>{body}</main></body></html>",
            escape(lang),
            escape(&title)
        );
        ctx.progress(&f.id, 0.3, "render", 1, 1);
        let dir = ctx.file_dir(&f.id)?;
        let out = if format == "html" {
            let out = dir.join("result.html");
            write_file(&out, page.as_bytes())?;
            out
        } else {
            std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
            let tmp = ctx.work_dir.join(format!("doc-{}.html", f.id));
            // <base> so images next to the Markdown file are found.
            std::fs::write(&tmp, edge::inject(&page, &src_dir, "")).map_err(super::io_err)?;
            let out = dir.join("result.pdf");
            edge::print(ctx, &f.id, &tmp, &out, Duration::from_secs(120))?;
            out
        };
        ctx.progress(&f.id, 1.0, "render", 1, 1);
        ctx.emit_output(&f.id, &out, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn decode(raw: &[u8]) -> String {
    crate::common::text::decode(raw)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// YAML front matter (`---` … `---` at the very top, as Obsidian, Jekyll and Hugo write
/// it) is not part of the text: it is left out, and its `title:` names the document.
pub(crate) fn front_matter(md: &str) -> (String, Option<String>) {
    let body = md.strip_prefix('\u{feff}').unwrap_or(md);
    let Some(rest) = body.strip_prefix("---\n").or_else(|| body.strip_prefix("---\r\n")) else { return (md.to_string(), None) };
    let mut offset = 0;
    for line in rest.split_inclusive('\n').take(200) {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            let meta = &rest[..offset];
            let title = meta.lines().find_map(|l| l.strip_prefix("title:")).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty());
            return (rest[offset + line.len()..].to_string(), title);
        }
        offset += line.len();
    }
    (md.to_string(), None)
}

/// A picture next to the Markdown file, as a data: URL (None when it isn't a local picture).
fn inline_picture(dir: &std::path::Path, url: &str) -> Option<String> {
    if url.contains("://") || url.starts_with("data:") || url.starts_with('#') {
        return None;
    }
    let rel = url.split(['?', '#']).next()?;
    let rel = percent_decode(rel);
    let path = dir.join(rel.trim_start_matches(['/', '\\']));
    let mime = match path.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        _ => return None,
    };
    if std::fs::metadata(&path).ok()?.len() > 25 * 1024 * 1024 {
        return None;
    }
    Some(format!("data:{mime};base64,{}", base64(&std::fs::read(&path).ok()?)))
}

pub(crate) fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            out.push(if k <= c.len() { A[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
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

/// Plain text: paragraphs at blank lines, line breaks kept.
fn text_to_html(s: &str) -> String {
    s.replace("\r\n", "\n")
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", escape(p.trim_end()).replace('\n', "<br>")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Markdown with tables, footnotes, task lists and strikethrough; headings get ids,
/// and with `toc` a list of them comes first.
fn md_to_html(md: &str, toc: bool, inline_from: Option<&std::path::Path>) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_SMART_PUNCTUATION;
    let mut heads: Vec<(u8, String, String)> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut in_head: Option<(HeadingLevel, usize, String)> = None;
    for ev in Parser::new_ext(md, opts) {
        match &ev {
            Event::Start(Tag::Heading { level, .. }) => {
                in_head = Some((*level, events.len(), String::new()));
                events.push(ev);
            }
            Event::Text(t) | Event::Code(t) if in_head.is_some() => {
                if let Some((_, _, s)) = in_head.as_mut() {
                    s.push_str(t);
                }
                events.push(ev);
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, at, text)) = in_head.take() {
                    let id = format!("h-{}", heads.len() + 1);
                    events[at] = Event::Start(Tag::Heading { level, id: Some(id.clone().into()), classes: vec![], attrs: vec![] });
                    heads.push((level as u8, text, id));
                }
                events.push(ev);
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) if inline_from.is_some() => {
                let url = inline_picture(inline_from.unwrap(), dest_url).map(Into::into).unwrap_or_else(|| dest_url.clone());
                events.push(Event::Start(Tag::Image { link_type: *link_type, dest_url: url, title: title.clone(), id: id.clone() }));
            }
            _ => events.push(ev),
        }
    }
    let mut out = String::new();
    if toc && heads.len() > 1 {
        out.push_str("<nav class=\"toc\"><ol>");
        let min = heads.iter().map(|h| h.0).min().unwrap_or(1);
        for (lvl, text, id) in heads.iter().filter(|h| h.0 <= min + 1) {
            out.push_str(&format!("<li class=\"l{}\"><a href=\"#{id}\">{}</a></li>", lvl - min, escape(text)));
        }
        out.push_str("</ol></nav>");
    }
    html::push_html(&mut out, events.into_iter());
    out
}

fn theme(name: &str) -> &'static str {
    match name {
        "book" => "body{font-family:Georgia,'Times New Roman',serif;font-size:12pt;line-height:1.6;color:#1b1b1b;margin:0}main{max-width:40em;margin:0 auto;padding:2em 1.5em}h1,h2,h3{font-weight:normal;line-height:1.25}h1{font-size:2em;text-align:center;margin:1.2em 0 .8em}h2{font-size:1.45em;margin-top:1.6em}p{text-align:justify;hyphens:auto}code,pre{font-family:Consolas,monospace;font-size:.9em}pre{background:#f5f2ea;padding:.8em;overflow:auto}blockquote{font-style:italic;color:#444;border-left:3px solid #c9bfa8;margin-left:0;padding-left:1em}table{border-collapse:collapse}td,th{border:1px solid #bbb;padding:.3em .6em}img{max-width:100%}.toc ol{list-style:none;padding:0}.toc .l1{padding-left:1.5em}",
        "compact" => "body{font-family:'Segoe UI',Arial,sans-serif;font-size:10pt;line-height:1.4;color:#111;margin:0}main{padding:1em}h1{font-size:1.6em;margin:.4em 0}h2{font-size:1.25em;margin:.8em 0 .3em}h3{font-size:1.05em}p,ul,ol{margin:.35em 0}code,pre{font-family:Consolas,monospace;font-size:.92em;background:#f3f4f6}pre{padding:.5em;overflow:auto}table{border-collapse:collapse}td,th{border:1px solid #ccc;padding:.2em .45em}img{max-width:100%}.toc ol{padding-left:1.2em}",
        _ => "body{font-family:'Segoe UI',Arial,sans-serif;font-size:11pt;line-height:1.6;color:#1f2328;margin:0}main{max-width:46em;margin:0 auto;padding:2em 1.5em}h1,h2{border-bottom:1px solid #d8dee4;padding-bottom:.25em}h1{font-size:1.9em}h2{font-size:1.45em;margin-top:1.5em}a{color:#0b5cad}code{font-family:Consolas,monospace;background:#f2f4f7;padding:.1em .35em;border-radius:4px;font-size:.92em}pre{background:#f6f8fa;padding:1em;border-radius:6px;overflow:auto}pre code{background:none;padding:0}blockquote{color:#59636e;border-left:4px solid #d1d9e0;margin-left:0;padding-left:1em}table{border-collapse:collapse;margin:1em 0}td,th{border:1px solid #d1d9e0;padding:.4em .8em}th{background:#f6f8fa}img{max-width:100%}.toc{background:#f6f8fa;border-radius:6px;padding:.6em 1em;margin-bottom:2em}.toc ol{margin:0;padding-left:1.2em}.toc .l1{margin-left:1em}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_and_base64() {
        let (body, title) = front_matter("---
title: \"Raport\"
tags: [a]
---
# Unu
");
        assert_eq!(body, "# Unu
");
        assert_eq!(title.as_deref(), Some("Raport"));
        assert_eq!(front_matter("# Fără
---
").1, None);
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(percent_decode("poz%C4%83%20mea.png"), "poză mea.png");
    }
}
