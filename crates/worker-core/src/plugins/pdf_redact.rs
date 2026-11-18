//! Redact a PDF for real: names, numbers and patterns found in the text (e-mails,
//! phone numbers, IBANs, card numbers, ID numbers) and boxes drawn in the app become
//! black boxes, and what was under them leaves the file (PyMuPDF, workers/python/
//! pdf-redact/redact.py). Hidden data (metadata, attachments, comments) can go too.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{img, pdf};
use contract::{codes, t, FileError, FileRef, Request};
use serde_json::{json, Value};
use std::path::Path;

pub struct PdfRedact;

impl Plugin for PdfRedact {
    fn id(&self) -> &'static str {
        "pdf-redact"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Acoperire date în PDF", "Redact PDF"),
            "summary": t("Ascunde de tot nume și numere", "Hide names and numbers for good"),
            "keywords": { "ro": ["anonimizare", "ascunde date", "date personale", "gdpr", "înnegrește"], "en": ["redact", "anonymize", "hide data", "personal data", "black out"] },
            "category": "pdf",
            "icon": "eye-off",
            "requires": [ { "engine": "python" }, { "engine": "pymupdf" } ],
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["needsPassword", "pageCount", "isSigned", "pages"] },
            "ui": { "preview": "none", "estimate": false, "editor": "pdf", "editorMode": "redact" },
            "resources": { "class": "light", "diskFactor": 1.2 },
            "presets": [
                { "id": "personal", "label": t("Date personale", "Personal data"), "default": true,
                  "values": { "emails": true, "phones": true, "ibans": true, "cards": true, "idNumbers": true } },
                { "id": "words", "label": t("Doar cuvintele mele", "Only my words"),
                  "values": { "emails": false, "phones": false, "ibans": false, "cards": false, "idNumbers": false } }
            ],
            "groups": [ { "id": "find", "label": t("Ce caut singur", "What to find by itself"), "order": 1 } ],
            "fields": [
                { "key": "terms", "type": "text", "default": "", "label": t("Cuvinte de acoperit", "Words to black out"),
                  "placeholder": t("de exemplu Ana Popescu; Str. Lungă 12", "for example Ana Popescu; 12 Long St"),
                  "help": t("Despărțite prin punct și virgulă. Fără deosebire între litere mari și mici.", "Separated by semicolons. Capitals or not, both are found.") },
                { "key": "emails", "type": "toggle", "default": true, "group": "find", "label": t("Adrese de e-mail", "E-mail addresses") },
                { "key": "phones", "type": "toggle", "default": true, "group": "find", "label": t("Numere de telefon", "Phone numbers") },
                { "key": "ibans", "type": "toggle", "default": true, "group": "find", "label": t("Conturi IBAN", "IBAN accounts") },
                { "key": "cards", "type": "toggle", "default": true, "group": "find", "label": t("Numere de card", "Card numbers") },
                { "key": "idNumbers", "type": "toggle", "default": true, "group": "find", "label": t("Coduri numerice lungi (CNP, CUI…)", "Long ID numbers (national, tax)") },
                { "key": "color", "type": "color", "default": "#000000", "label": t("Culoarea casetelor", "Box color") },
                { "key": "scrub", "type": "toggle", "default": true, "label": t("Șterge și datele ascunse (autor, atașamente, comentarii)", "Also remove hidden data (author, attachments, comments)") }
            ],
            "rules": [
                { "id": "signature_lost", "when": { "file": "isSigned", "eq": true }, "severity": "confirm",
                  "message": t("Semnăturile digitale nu vor mai fi valide.", "Digital signatures won't be valid anymore.") },
                { "id": "check", "severity": "info",
                  "message": t("Verifică rezultatul: textul din imagini (scanări) nu poate fi găsit singur; trage casete peste el.",
                               "Check the result: text inside pictures (scans) can't be found by itself; draw boxes over it.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_acoperit", "{name}_redacted") } },
            "errors": {
                "nothing_found": t("Nu am găsit nimic de acoperit în acest fișier.", "Nothing to black out was found in this file.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, mut props, code) = pdf::analyze(f, req);
        if status == "ok" {
            if let Ok(doc) = pdf::open_file(f, req) {
                let pages: Vec<Value> = doc.get_pages().values().map(|id| {
                    let (w, h) = pdf::displayed_size(&doc, *id);
                    json!([w.round(), h.round()])
                }).collect();
                props.insert("pages".into(), json!(pages));
            }
        }
        Analysis { status, props, warnings: vec![], code }
    }

    /// Pages for the editor (where boxes are drawn), as the fill-and-sign tool shows them.
    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        super::pdf_edit::PdfEdit.preview(req)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        use std::io::BufRead;
        let req = ctx.req;
        let in_bytes = file_size(f.input_path());
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let engine = |name: &str| req.engines.get(name).map(|e| e.path.clone()).ok_or_else(|| FileError::with(codes::ENGINE_MISSING, json!({ "component": name })));
        let python = engine("python")?.join("python.exe");
        let paths = engine("pymupdf")?.join("site-packages").to_string_lossy().into_owned();
        // Beside the workers: <app>\plugins\pdf-redact\redact.py.
        let script = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().and_then(Path::parent).map(|d| d.join("plugins").join("pdf-redact").join("redact.py")))
            .filter(|p| p.is_file())
            .ok_or_else(|| FileError::with(codes::ENGINE_MISSING, json!({ "message": "redact.py" })))?;
        let terms: Vec<String> = req.str_or("terms", "").split([';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let mut patterns = Vec::new();
        for (key, name) in [("emails", "email"), ("phones", "phone"), ("ibans", "iban"), ("cards", "card"), ("idNumbers", "idnum")] {
            if req.bool_or(key, true) {
                patterns.push(name);
            }
        }
        let boxes: Vec<Value> = f.item.get("edits").and_then(Value::as_array).cloned().unwrap_or_default()
            .into_iter().filter(|e| e["type"] == "redact").collect();
        let c = img::parse_color(req.str_or("color", "#000000"));
        let spec = json!({ "terms": terms, "patterns": patterns, "boxes": boxes, "color": c, "scrub": req.bool_or("scrub", true),
                           "password": req.secrets.files.get(&f.id) });
        std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
        let spec_file = ctx.work_dir.join(format!("redact-{}.json", f.id));
        write_file(&spec_file, spec.to_string().as_bytes())?;
        let out = ctx.file_dir(&f.id)?.join("result.pdf");

        let mut cmd = std::process::Command::new(&python);
        cmd.arg("-I").arg(&script).arg(&paths).arg(&spec_file).arg(f.input_path()).arg(&out);
        cmd.env("PYTHONUTF8", "1").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| FileError::with(codes::ENGINE_FAILED, json!({ "message": e.to_string() })))?;
        let stdout = child.stdout.take().expect("piped");
        let last = std::sync::Arc::new(std::sync::Mutex::new((0.0f64, String::new())));
        let reader = {
            let last = last.clone();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                    let mut l = last.lock().unwrap();
                    if let Some(p) = line.strip_prefix("progress ").and_then(|p| p.trim().parse::<f64>().ok()) {
                        l.0 = p;
                    } else {
                        l.1 = line;
                    }
                }
            })
        };
        let limit = crate::common::proc::limit_for(in_bytes, std::time::Duration::from_secs(120), std::time::Duration::from_secs(2), std::time::Duration::from_secs(3600));
        let status = crate::common::proc::wait(&mut child, Some(&ctx.cancel), "PyMuPDF", limit, || {
            let p = last.lock().unwrap().0;
            ctx.progress(&f.id, 0.05 + 0.9 * p, "redact", 2, 2);
        })?;
        let _ = reader.join();
        let _ = std::fs::remove_file(&spec_file);
        let end = last.lock().unwrap().1.clone();
        if !status.success() || !end.starts_with("done") {
            let _ = std::fs::remove_file(&out);
            return Err(match end.split_whitespace().nth(1) {
                Some("password_required") => FileError::new(codes::PASSWORD_REQUIRED),
                _ => FileError::with(codes::ENGINE_FAILED, json!({ "message": end.trim_start_matches("error ").trim_start_matches("failed ") })),
            });
        }
        let count: u64 = end.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
        if count == 0 && !req.bool_or("scrub", true) {
            let _ = std::fs::remove_file(&out);
            return Err(FileError::skipped("nothing_found", json!({})));
        }
        if count == 0 {
            ctx.out.warning(Some(&f.id), "nothing_found", None);
        }
        ctx.progress(&f.id, 1.0, "redact", 2, 2);
        ctx.emit_output(&f.id, &out, "pdf", json!({ "count": count }), in_bytes);
        Ok(Value::Null)
    }
}
