//! HTML to PDF: a saved web page or an HTML file, printed by Edge with the paper,
//! margins and backgrounds chosen here. Relative images and styles keep working.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::edge;
use contract::{opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::time::Duration;

pub struct HtmlToPdf;

/// The page fields shared with Markdown and text.
pub fn page_fields() -> Vec<Value> {
    vec![
        json!({ "key": "pageSize", "type": "select", "label": t("Hârtie", "Paper"), "default": "a4", "display": "chips",
                "options": [ opt("a4", "A4", "A4"), opt("letter", "Letter", "Letter"), opt("a3", "A3", "A3"), opt("a5", "A5", "A5") ] }),
        json!({ "key": "margin", "type": "select", "label": t("Margini", "Margins"), "default": 15,
                "options": [ opt(0, "Fără", "None"), opt(8, "Mici", "Small"), opt(15, "Normale", "Normal"), opt(25, "Mari", "Large") ] }),
        json!({ "key": "landscape", "type": "toggle", "default": false, "label": t("Pagină culcată", "Landscape") }),
        json!({ "key": "backgrounds", "type": "toggle", "default": true, "label": t("Tipărește culorile de fundal", "Print background colors") }),
    ]
}

pub fn page_css(req: &Request) -> String {
    edge::page_css(req.str_or("pageSize", "a4"), req.bool_or("landscape", false), req.f64_or("margin", 15.0), req.bool_or("backgrounds", true))
}

impl Plugin for HtmlToPdf {
    fn id(&self) -> &'static str {
        "html-to-pdf"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("HTML în PDF", "HTML to PDF"),
            "summary": t("Pagini web de păstrat", "Web pages to keep"),
            "keywords": { "ro": ["html", "pagină web", "salvează pagina", "pdf"], "en": ["html", "web page", "save page", "pdf"] },
            "category": "document",
            "icon": "code",
            "input": { "extensions": ["html", "htm", "xhtml", "svg"], "multiple": true, "folders": true, "mode": "perFile", "textEntry": "html" },
            "output": { "kind": "perInput", "extensions": ["pdf"], "replaceOriginalAllowed": false },
            "analysis": { "fast": [] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [],
            "groups": [],
            "fields": page_fields(),
            "rules": [
                { "id": "edge", "severity": "info", "when": { "field": "landscape", "in": [true, false] },
                  "message": t("Pagina e tipărită de Microsoft Edge, care e deja pe calculator. Fișierul nu e trimis nicăieri; pozele și stilurile pe care pagina le ia de pe internet se încarcă, ca într-un browser.",
                               "The page is printed by Microsoft Edge, already on your PC. Your file isn't sent anywhere; pictures and styles the page takes from the internet are loaded, as in a browser.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {}
        })
    }

    fn analyze(&self, _f: &FileRef, _req: &Request) -> Analysis {
        Analysis::ok(Map::new())
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "render", 1, 1);
        let raw = std::fs::read(path).map_err(super::io_err)?;
        let html = decode_html(&raw);
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
        let tmp = ctx.work_dir.join(format!("page-{}.html", f.id));
        // UTF-8 with a byte order mark: the mark outranks the page's own <meta charset>,
        // which named the encoding of the original bytes, not of this copy.
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(edge::inject(&html, &dir, &page_css(ctx.req)).as_bytes());
        std::fs::write(&tmp, data).map_err(super::io_err)?;
        ctx.progress(&f.id, 0.3, "render", 1, 1);
        let out = ctx.file_dir(&f.id)?.join("result.pdf");
        edge::print(ctx, &f.id, &tmp, &out, Duration::from_secs(120))?;
        ctx.progress(&f.id, 1.0, "render", 1, 1);
        ctx.emit_output(&f.id, &out, "pdf", json!({}), in_bytes);
        Ok(Value::Null)
    }
}

/// The page's text: a byte order mark, else UTF-8 when the bytes are valid UTF-8, else the
/// encoding the page declares (`<meta charset>`, `http-equiv`), else the best guess. Pages
/// saved in Windows-1250 came out as "Pďż˝ine".
fn decode_html(raw: &[u8]) -> String {
    use encoding_rs::Encoding;
    if let Some((enc, bom)) = Encoding::for_bom(raw) {
        return enc.decode_without_bom_handling(&raw[bom..]).0.into_owned();
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let head = String::from_utf8_lossy(&raw[..raw.len().min(4096)]).to_ascii_lowercase();
    let declared = head.find("charset").and_then(|i| {
        let rest = head[i + 7..].trim_start().strip_prefix('=')?.trim_start().trim_start_matches(['"', '\'']);
        let label: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        Encoding::for_label(label.as_bytes())
    });
    match declared {
        Some(enc) if enc != encoding_rs::UTF_8 => enc.decode_without_bom_handling(raw).0.into_owned(),
        _ => crate::common::text::decode(raw),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decode_html_by_declared_charset() {
        let (bytes, _, _) = encoding_rs::WINDOWS_1250.encode("<meta charset=\"windows-1250\"><p>Pâine şi ţuică</p>");
        assert!(super::decode_html(&bytes).contains("Pâine şi ţuică"));
        assert_eq!(super::decode_html("<p>Ștefan</p>".as_bytes()), "<p>Ștefan</p>");
    }
}
