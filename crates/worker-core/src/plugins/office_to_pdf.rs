//! Excel and PowerPoint to PDF (and OpenDocument, CSV): every sheet or slide, optionally
//! a sheet per page or the speaker notes. Microsoft Office makes them when it is installed
//! (office-worker, entry `office`); this is the LibreOffice entry, and the analysis.
//! CSV files stay with LibreOffice (Excel would read them in the PC's own encoding and
//! separator), and so do Excel files as PDF/A (Excel can't be asked for it).

use super::word_to_pdf_lo::{convert_with_lo, convert_with_lo_as};
use super::{file_size, Analysis, Ctx, Plugin};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::io::Read;

pub struct OfficeToPdf;

const SHEETS: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv"];
const SLIDES: &[&str] = &["pptx", "pptm", "ppt", "pps", "ppsx", "odp"];

impl Plugin for OfficeToPdf {
    fn id(&self) -> &'static str {
        "office-to-pdf"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = SHEETS.iter().chain(SLIDES).copied().collect();
        json!({
            "name": t("Excel și PowerPoint în PDF", "Excel and PowerPoint to PDF"),
            "summary": t("Tabele și prezentări", "Sheets and slides"),
            "keywords": { "ro": ["excel", "powerpoint", "xlsx", "pptx", "prezentare", "tabel"], "en": ["excel", "powerpoint", "xlsx", "pptx", "presentation", "spreadsheet"] },
            "category": "document",
            "icon": "presentation",
            "entry": [
                { "id": "office", "kind": "dotnet", "worker": "office-worker", "command": "office-to-pdf",
                  "when": { "all": [
                      { "field": "engine", "ne": "libreoffice" },
                      { "any": [ { "env": "office.excel", "eq": "ok" }, { "env": "office.powerpoint", "eq": "ok" } ] },
                      { "any": [ { "env": "office.excel", "eq": "ok" }, { "file": "kind", "eq": "sheets", "quant": "none" } ] },
                      { "any": [ { "env": "office.powerpoint", "eq": "ok" }, { "file": "kind", "eq": "slides", "quant": "none" } ] },
                      { "file": "format", "eq": "csv", "quant": "none" },
                      { "not": { "all": [ { "field": "pdfA", "eq": true }, { "file": "kind", "eq": "sheets", "quant": "any" } ] } }
                  ] } },
                { "id": "libreoffice", "kind": "worker", "worker": "worker-core", "command": "office-to-pdf" }
            ],
            "analyzeEntry": { "kind": "worker", "worker": "worker-core", "command": "office-to-pdf" },
            "requires": [ { "engine": "libreoffice", "when": { "env": "resolvedEngine", "eq": "libreoffice" } } ],
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"],
                        "nameVariables": [ { "id": "pages", "label": t("număr de pagini", "page count") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "kind", "needsPassword"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "office", "diskFactor": 1.5 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "quality", "type": "select", "label": t("Pentru", "For"), "default": "print",
                  "options": [ opt("print", "Tipar (calitate maximă)", "Print (best quality)"), opt("screen", "Ecran (fișier mai mic)", "Screen (smaller file)") ] },
                { "key": "sheetPerPage", "type": "toggle", "default": false,
                  "label": t("Fiecare foaie pe o singură pagină", "Each sheet on a single page"),
                  "help": t("Tabelele mari nu se mai rup pe mai multe pagini.", "Large tables are no longer split across pages."),
                  "showIf": { "file": "kind", "eq": "sheets", "quant": "any" } },
                { "key": "notes", "type": "toggle", "default": false,
                  "label": t("Cu notițele vorbitorului", "With the speaker notes"),
                  "showIf": { "file": "kind", "eq": "slides", "quant": "any" } },
                { "key": "pdfA", "type": "toggle", "advanced": true, "group": "more", "default": false, "label": t("PDF/A pentru arhivare", "PDF/A for archiving") },
                { "key": "engine", "type": "select", "advanced": true, "group": "more", "label": t("Cu ce program", "With which program"), "default": "auto",
                  "showIf": { "any": [ { "env": "office.excel", "eq": "ok" }, { "env": "office.powerpoint", "eq": "ok" } ] },
                  "options": [ opt("auto", "Microsoft Office (recomandat)", "Microsoft Office (recommended)"), opt("libreoffice", "LibreOffice", "LibreOffice") ] }
            ],
            "rules": [
                { "id": "lo", "severity": "info", "when": { "env": "resolvedEngine", "eq": "libreoffice" },
                  "message": t("Conversia o face LibreOffice, descărcat o singură dată. Fișierele complicate pot arăta puțin diferit decât în Office.",
                               "LibreOffice does the conversion, downloaded once. Complex files may look slightly different than in Office.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "office_not_ready": t("Excel sau PowerPoint nu a putut porni. Deschide-l o dată, închide orice fereastră care apare (activare, licență), apoi încearcă din nou.",
                                      "Excel or PowerPoint couldn't start. Open it once, close any window it shows (activation, license), then try again."),
                "nothing_to_print": t("Foile de calcul sunt goale: nu e nimic de pus în PDF.", "The sheets are empty: there is nothing to put in the PDF.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let path = f.input_path();
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let kind = if SHEETS.contains(&ext.as_str()) {
            "sheets"
        } else if SLIDES.contains(&ext.as_str()) {
            "slides"
        } else {
            return Analysis::status("unsupported", codes::UNSUPPORTED_FILE);
        };
        let mut p = Map::new();
        p.insert("format".into(), json!(ext));
        p.insert("kind".into(), json!(kind));
        let mut head = [0u8; 4];
        let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
        // A modern Office file inside an OLE container is an encrypted one.
        let ooxml = matches!(ext.as_str(), "xlsx" | "xlsm" | "xlsb" | "pptx" | "pptm" | "ppsx");
        if n == 4 && ooxml && head == [0xD0, 0xCF, 0x11, 0xE0] {
            // A typed password is checked here, so a wrong one is said at once.
            let code = match req.secrets.files.get(&f.id).map(|pw| crate::common::offcrypto::unlock(path, pw)) {
                Some(Ok(_)) => None,
                Some(Err(c)) if c != codes::WRONG_PASSWORD => return Analysis::status("corrupt", c),
                Some(Err(c)) => Some(c),
                None => Some(codes::PASSWORD_REQUIRED),
            };
            if let Some(code) = code {
                p.insert("needsPassword".into(), json!(true));
                return Analysis { status: "locked", props: p, warnings: vec![], code: Some(code.into()) };
            }
        }
        // Office 97-2003 with a password: asked for here, checked by Office when it converts.
        if matches!(ext.as_str(), "xls" | "ppt" | "pps") && crate::common::offcrypto::legacy_encrypted(path) {
            p.insert("isEncrypted".into(), json!(true));
            if !req.secrets.files.contains_key(&f.id) {
                p.insert("needsPassword".into(), json!(true));
                return Analysis { status: "locked", props: p, warnings: vec![], code: Some(codes::PASSWORD_REQUIRED.into()) };
            }
        }
        // The container each format must have: a zip (Office 2007+, OpenDocument) or an OLE
        // file (Office 97-2003). Anything else is damaged; LibreOffice would print its bytes.
        let zip = head.starts_with(b"PK");
        let ole = head == [0xD0, 0xCF, 0x11, 0xE0];
        let fits = match ext.as_str() {
            "csv" => true,
            "xls" | "ppt" | "pps" => ole,
            // An encrypted Office 2007+ file is an OLE one (its password is handled above).
            "xlsx" | "xlsm" | "xlsb" | "pptx" | "pptm" | "ppsx" => zip || ole,
            _ => zip,
        };
        if !fits {
            return Analysis::status("corrupt", codes::CORRUPT_FILE);
        }
        p.insert("needsPassword".into(), json!(false));
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let input = f.input_path();
        let in_bytes = file_size(input);
        let ext = input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let sheets = SHEETS.contains(&ext.as_str());
        let dir = ctx.file_dir(&f.id)?;
        let o = |t: &str, v: &str| json!({ "type": t, "value": v });
        let mut opts = Map::new();
        if req.str_or("quality", "print") == "screen" {
            opts.insert("ReduceImageResolution".into(), o("boolean", "true"));
            opts.insert("MaxImageResolution".into(), o("long", "150"));
            opts.insert("Quality".into(), o("long", "80"));
        }
        if req.bool_or("pdfA", false) {
            opts.insert("SelectPdfVersion".into(), o("long", "2"));
        }
        if sheets && req.bool_or("sheetPerPage", false) {
            opts.insert("SinglePageSheets".into(), o("boolean", "true"));
        }
        if !sheets && req.bool_or("notes", false) {
            opts.insert("ExportNotesPages".into(), o("boolean", "true"));
        }
        let base = if sheets { "calc_pdf_Export" } else { "impress_pdf_Export" };
        let filter = if opts.is_empty() { format!("pdf:{base}") } else { format!("pdf:{base}:{}", Value::Object(opts)) };
        let produced = if ext == "csv" {
            // LibreOffice would read a CSV as UTF-8 split on commas: a Windows-1250 file
            // split on semicolons came out as garbage. It gets a UTF-8 copy and is told the
            // separator; every value stays as written (0042, 12,5).
            let raw = std::fs::read(input).map_err(super::io_err)?;
            let text = crate::common::text::decode(&raw);
            let delim = super::tabular::csv_delimiter(&text);
            let columns = text.lines().take(200).map(|l| l.bytes().filter(|b| *b == delim).count() + 1).max().unwrap_or(1).min(1024);
            let formats: Vec<String> = (1..=columns).map(|c| format!("{c}/2")).collect();
            let copy_dir = ctx.work_dir.join(format!("{}-csv", f.id));
            std::fs::create_dir_all(&copy_dir).map_err(super::io_err)?;
            let copy = copy_dir.join(input.file_name().unwrap_or_default());
            std::fs::write(&copy, text.as_bytes()).map_err(super::io_err)?;
            let infilter = format!("CSV:{},34,76,1,{}", delim, formats.join("/"));
            let r = convert_with_lo_as(ctx, f, Some((&copy, &infilter)), &filter, &dir);
            let _ = std::fs::remove_dir_all(&copy_dir);
            r?
        } else {
            convert_with_lo(ctx, f, &filter, &dir)?
        };
        let out = dir.join("result.pdf");
        std::fs::rename(&produced, &out).map_err(super::io_err)?;
        if !req.bool_or("pdfA", false) {
            super::word_to_pdf_lo::compact(&out);
        }
        let doc = crate::common::pdf::open(&out, None).ok();
        let pages = doc.as_ref().map(|d| d.get_pages().len()).unwrap_or(0);
        // A workbook with nothing in it: Excel says there is nothing to print, rather than
        // making a blank page.
        if sheets && doc.as_ref().is_some_and(draws_nothing) {
            let _ = std::fs::remove_file(&out);
            return Err(FileError::new("nothing_to_print"));
        }
        ctx.progress(&f.id, 1.0, "export", 2, 2);
        ctx.emit_output(&f.id, &out, "pdf", json!({ "pages": pages }), in_bytes);
        Ok(json!({ "engine": "libreoffice" }))
    }
}

/// No page paints anything (text, shapes, pictures): only clipping and settings.
fn draws_nothing(doc: &lopdf::Document) -> bool {
    const PAINT: &[&str] = &["f", "F", "f*", "B", "B*", "b", "b*", "S", "s", "Tj", "TJ", "'", "\"", "Do", "sh", "BI", "EI"];
    doc.get_pages().values().all(|id| match lopdf::content::Content::decode(&doc.get_page_content(*id)) {
        Ok(c) => !c.operations.iter().any(|op| PAINT.contains(&op.operator.as_str())),
        Err(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entry the app would pick for a batch (same rule as the app: the first that holds).
    fn engine(values: Value, files: &[(&str, &str)], excel: bool, powerpoint: bool) -> String {
        let m = OfficeToPdf.manifest();
        let mut vals = Map::new();
        vals.insert("engine".into(), json!("auto"));
        vals.insert("pdfA".into(), json!(false));
        vals.extend(values.as_object().unwrap().clone());
        let files: Vec<Map<String, Value>> = files.iter().map(|(f, k)| json!({ "format": f, "kind": k }).as_object().unwrap().clone()).collect();
        let ok = |b: bool| json!(if b { "ok" } else { "missing" });
        let env = json!({ "office.excel": ok(excel), "office.powerpoint": ok(powerpoint) }).as_object().unwrap().clone();
        let ctx = contract::cond::Ctx { values: &vals, files: &files, env: &env };
        let e = m["entry"].as_array().unwrap().iter().find(|e| contract::cond::eval(e.get("when"), &ctx)).unwrap();
        e["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn office_when_it_can() {
        let both = [("xlsx", "sheets"), ("pptx", "slides")];
        assert_eq!(engine(json!({}), &both, true, true), "office");
        assert_eq!(engine(json!({}), &[], true, true), "office");
        assert_eq!(engine(json!({}), &[], false, false), "libreoffice");
        assert_eq!(engine(json!({ "engine": "libreoffice" }), &both, true, true), "libreoffice");
        // Only PowerPoint: slides with it, a batch with sheets with LibreOffice.
        assert_eq!(engine(json!({}), &[("pptx", "slides")], false, true), "office");
        assert_eq!(engine(json!({}), &both, false, true), "libreoffice");
        // CSV, and Excel files as PDF/A, stay with LibreOffice; slides as PDF/A don't.
        assert_eq!(engine(json!({}), &[("csv", "sheets")], true, true), "libreoffice");
        assert_eq!(engine(json!({ "pdfA": true }), &[("xlsx", "sheets")], true, true), "libreoffice");
        assert_eq!(engine(json!({ "pdfA": true }), &[("pptx", "slides")], true, true), "office");
    }
}
