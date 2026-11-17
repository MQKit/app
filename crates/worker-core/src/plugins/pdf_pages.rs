//! Rotate, delete and reorder PDF pages, on one file or a batch: by ranges, odd/even,
//! portrait/landscape, reversing, or fixing a front-then-back scan.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::pdf;
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use lopdf::{Object, ObjectId};
use serde_json::{json, Value};

pub struct PdfPages;

impl Plugin for PdfPages {
    fn id(&self) -> &'static str {
        "pdf-pages"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Rotire și ordine pagini PDF", "Rotate and reorder PDF"),
            "summary": t("Rotește, șterge, mută pagini", "Rotate, delete, move pages"),
            "keywords": { "ro": ["rotește", "întoarce", "șterge pagini", "ordinea paginilor", "inversează"], "en": ["rotate", "turn", "delete pages", "page order", "reverse"] },
            "category": "pdf",
            "icon": "rotate",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["needsPassword", "pageCount", "isSigned"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.2 },
            "presets": [
                { "id": "right", "label": t("Rotește tot la dreapta", "Rotate all right"), "default": true,
                  "values": { "rotation": 90, "applyTo": "all", "deletePages": "", "order": "keep" } },
                { "id": "left", "label": t("Rotește tot la stânga", "Rotate all left"), "values": { "rotation": -90, "applyTo": "all", "deletePages": "", "order": "keep" } },
                { "id": "landscape", "label": t("Îndreaptă paginile culcate", "Straighten sideways pages"),
                  "values": { "rotation": 90, "applyTo": "landscape", "deletePages": "", "order": "keep" } },
                { "id": "reverse", "label": t("Inversează ordinea", "Reverse the order"), "values": { "rotation": 0, "deletePages": "", "order": "reverse" } },
                { "id": "scan", "label": t("Scanare: fețe, apoi versouri", "Scan: fronts, then backs"),
                  "values": { "rotation": 0, "deletePages": "", "order": "interleave" } },
                { "id": "noFirst", "label": t("Fără prima pagină", "Without the first page"), "values": { "rotation": 0, "deletePages": "1", "order": "keep" } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "rotation", "type": "select", "label": t("Rotire", "Rotate"), "default": 90, "display": "chips",
                  "options": [ opt(0, "Fără", "None"), opt(90, "La dreapta", "Right"), opt(-90, "La stânga", "Left"), opt(180, "Cu susul în jos", "Upside down") ] },
                { "key": "applyTo", "type": "select", "label": t("Ce pagini rotesc", "Which pages to rotate"), "default": "all",
                  "options": [ opt("all", "Toate", "All"), opt("range", "Doar unele", "Only some"), opt("odd", "Impare", "Odd"), opt("even", "Pare", "Even"),
                               opt("landscape", "Doar cele culcate", "Only sideways ones"), opt("portrait", "Doar cele în picioare", "Only upright ones") ],
                  "showIf": { "field": "rotation", "ne": 0 } },
                { "key": "range", "type": "pages", "syntax": "set", "label": t("Pagini de rotit", "Pages to rotate"), "default": "all",
                  "placeholder": t("de exemplu 1-3, 7", "for example 1-3, 7"),
                  "showIf": { "all": [ { "field": "rotation", "ne": 0 }, { "field": "applyTo", "eq": "range" } ] } },
                { "key": "deletePages", "type": "text", "label": t("Pagini de șters", "Pages to delete"), "default": "",
                  "placeholder": t("de exemplu 2, 5-7, z", "for example 2, 5-7, z"),
                  "help": t("z = ultima pagină; 1-z:even = toate paginile pare.", "z = last page; 1-z:even = every even page.") },
                { "key": "order", "type": "select", "label": t("Ordinea paginilor", "Page order"), "default": "keep",
                  "options": [ opt("keep", "Neschimbată", "Unchanged"), opt("reverse", "Inversată", "Reversed"),
                               opt("interleave", "Scanare față-verso (fețe, apoi versouri)", "Two-sided scan (fronts, then backs)"),
                               opt("custom", "Ordinea mea", "My own order") ] },
                { "key": "customOrder", "type": "pages", "syntax": "ordered", "label": t("Noua ordine", "New order"), "default": "all",
                  "placeholder": t("de exemplu 3, 1, 2, 4-z", "for example 3, 1, 2, 4-z"),
                  "help": t("Cu numerele paginilor din fișierul original.", "Using the page numbers of the original file."),
                  "showIf": { "field": "order", "eq": "custom" } },
                { "key": "rotationMode", "type": "select", "advanced": true, "group": "more", "default": "relative",
                  "label": t("Rotirea se adaugă sau înlocuiește", "Rotation adds or replaces"),
                  "options": [ opt("relative", "Se adaugă la cea existentă", "Adds to the current one"), opt("absolute", "Înlocuiește rotirea", "Replaces the rotation") ],
                  "showIf": { "field": "rotation", "ne": 0 } }
            ],
            "rules": [
                // Nothing chosen would only save a copy of the same pages.
                { "id": "nothing_to_change", "severity": "error",
                  "when": { "all": [ { "field": "rotation", "eq": 0 }, { "field": "deletePages", "eq": "" }, { "field": "order", "eq": "keep" } ] },
                  "message": t("Alege ce schimbi: rotirea, paginile de șters sau ordinea.", "Choose what to change: the rotation, pages to delete or the order.") },
                { "id": "signature_lost", "when": { "file": "isSigned", "eq": true }, "severity": "confirm",
                  "message": t("Semnăturile digitale nu vor mai fi valide.", "Digital signatures won't be valid anymore.") },
                { "id": "interleave_hint", "severity": "info", "when": { "field": "order", "eq": "interleave" },
                  "message": t("Pentru un teanc scanat întâi pe față, apoi întors și scanat pe verso: versourile vin în ordine inversă.",
                               "For a stack scanned on the fronts, then turned over and scanned on the backs: the backs come in reverse order.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_pagini", "{name}_pages") } },
            "errors": { "empty_selection": t("Nu rămâne nicio pagină.", "No pages are left.") }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let in_bytes = file_size(f.input_path());
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let mut doc = pdf::open_file(f, req)?;
        // Bookmarks and links by name keep working in the new file(s).
        pdf::resolve_named_dests(&mut doc);
        let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        let total = ids.len() as u32;
        let bad = |part: String| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": part, "pageCount": total }));
        for p in &ids {
            pdf::materialize_inherited(&mut doc, *p);
        }

        // Rotation, on the original page numbers.
        let delta = req.f64_or("rotation", 0.0) as i64;
        let mut rotated = 0;
        if delta != 0 {
            let chosen: Vec<u32> = match req.str_or("applyTo", "all") {
                "range" => pages::parse_set(req.str_or("range", "all"), total).map_err(bad)?,
                "odd" => (1..=total).filter(|p| p % 2 == 1).collect(),
                "even" => (1..=total).filter(|p| p % 2 == 0).collect(),
                which @ ("landscape" | "portrait") => (1..=total)
                    .filter(|p| {
                        let id = ids[(*p - 1) as usize];
                        let (w, h) = pdf::page_box(&doc, id).unwrap_or((1.0, 1.0));
                        let quarter = current_rotation(&doc, id).rem_euclid(180) == 90;
                        let wide = (w > h) != quarter;
                        (which == "landscape") == wide
                    })
                    .collect(),
                _ => (1..=total).collect(),
            };
            let absolute = req.str_or("rotationMode", "relative") == "absolute";
            for p in chosen {
                let id = ids[(p - 1) as usize];
                let now = current_rotation(&doc, id);
                let new = if absolute { delta } else { now + delta }.rem_euclid(360);
                if let Ok(d) = doc.get_dictionary_mut(id) {
                    d.set("Rotate", new);
                    rotated += 1;
                }
            }
        }

        // Deleting, then ordering.
        let deleted = match req.str_or("deletePages", "").trim() {
            "" => vec![],
            spec => pages::parse_set(spec, total).map_err(bad)?,
        };
        let kept: Vec<u32> = (1..=total).filter(|p| !deleted.contains(p)).collect();
        let order: Vec<u32> = match req.str_or("order", "keep") {
            "reverse" => kept.iter().rev().copied().collect(),
            "interleave" => {
                let half = kept.len().div_ceil(2);
                let (fronts, backs) = kept.split_at(half);
                let mut out = Vec::with_capacity(kept.len());
                let mut b = backs.iter().rev();
                for fr in fronts {
                    out.push(*fr);
                    if let Some(x) = b.next() {
                        out.push(*x);
                    }
                }
                out
            }
            "custom" => pages::parse_ordered(req.str_or("customOrder", "all"), total)
                .map_err(bad)?
                .into_iter()
                .filter(|p| !deleted.contains(p))
                .collect(),
            _ => kept,
        };
        if order.is_empty() {
            return Err(FileError::new("empty_selection"));
        }
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.5, "write", 2, 2);
        let page_ids: Vec<ObjectId> = order.iter().map(|p| ids[(*p - 1) as usize]).collect();
        let mut out_doc = pdf::extract_pages(&doc, &page_ids, true);
        let path = ctx.file_dir(&f.id)?.join("result.pdf");
        pdf::save(&mut out_doc, &path, false)?;
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        ctx.emit_output(&f.id, &path, "pdf", json!({ "pages": order.len() }), in_bytes);
        Ok(json!({ "rotated": rotated, "deleted": deleted.len(), "pages": order.len() }))
    }
}

fn current_rotation(doc: &lopdf::Document, id: ObjectId) -> i64 {
    doc.get_dictionary(id).ok().and_then(|d| d.get(b"Rotate").ok()).and_then(|o| match o {
        Object::Integer(i) => Some(*i),
        Object::Real(r) => Some(*r as i64),
        _ => None,
    }).unwrap_or(0)
}
