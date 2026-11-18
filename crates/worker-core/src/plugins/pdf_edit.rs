//! Fill and sign a PDF: its form fields filled in (with appearances drawn, so every
//! reader shows them), and text, a signature or a picture, ticks and crosses, white-out
//! and highlights placed on the pages where the app's editor put them. Optionally the
//! form is flattened: what was filled becomes part of the page.
//!
//! The editor sends, per file (`item`):
//!   form:  { "<field full name>": "text" | true/false | "<radio state>" }
//!   edits: [ { type, page, x, y, w, h, … } ] with x, y, w, h as fractions of the page as
//!          shown (after /Rotate), from its top-left corner.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{b64, font::PdfFont, img, pdf};
use contract::{t, FileError, FileRef, Request};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub struct PdfEdit;

impl Plugin for PdfEdit {
    fn id(&self) -> &'static str {
        "pdf-edit"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Completare și semnare PDF", "Fill and sign PDF"),
            "summary": t("Text, semnătură, formulare", "Text, signature, forms"),
            "keywords": { "ro": ["semnează", "completează", "formular", "editează pdf", "adaugă text"], "en": ["sign", "fill", "form", "edit pdf", "add text"] },
            "category": "pdf",
            "icon": "writing-sign",
            "input": { "extensions": ["pdf"], "multiple": false, "folders": false, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["needsPassword", "pageCount", "isSigned", "hasXfa", "fields", "pages"] },
            "ui": { "preview": "none", "estimate": false, "editor": "pdf" },
            "resources": { "class": "light", "diskFactor": 1.2 },
            "fields": [
                { "key": "flatten", "type": "toggle", "default": false, "label": t("Fixează câmpurile completate în pagină", "Make the filled fields part of the page"),
                  "help": t("Nu se mai pot modifica după aceea. Bine pentru trimis mai departe.", "They can't be changed afterwards. Good for sending on.") }
            ],
            "rules": [
                { "id": "signature_lost", "when": { "file": "isSigned", "eq": true }, "severity": "confirm",
                  "message": t("Semnăturile digitale nu vor mai fi valide.", "Digital signatures won't be valid anymore.") },
                { "id": "xfa", "when": { "all": [ { "file": "hasXfa", "eq": true }, { "file": "fields", "empty": true } ] }, "severity": "warning",
                  "message": t("Formularul e de tip XFA (Adobe LiveCycle): câmpurile lui nu se pot completa aici, dar poți scrie peste pagină.",
                               "The form is an XFA form (Adobe LiveCycle): its fields can't be filled here, but you can write on the page.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_completat", "{name}_filled") } },
            "errors": {
                "nothing_to_do": t("Nu ai adăugat nimic: completează un câmp sau pune ceva pe pagină.", "Nothing was added: fill a field or put something on a page."),
                "bad_picture": t("O imagine nu a putut fi deschisă.", "A picture couldn't be opened.")
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
                props.insert("fields".into(), json!(fields(&doc)));
            }
        }
        Analysis { status, props, warnings: vec![], code }
    }

    /// The page `options.page` (from 1) as a picture, `maxPx` on its longer side.
    fn preview(&self, req: &Request) -> Result<Option<Value>, FileError> {
        let Some(f) = super::preview_file(req) else { return Ok(None) };
        let max = req.target.as_ref().and_then(|t| t.get("maxPx")).and_then(Value::as_u64).unwrap_or(900).clamp(64, 1600) as f64;
        let reader = crate::common::winrt::Pdf::open(f.input_path(), req.secrets.files.get(&f.id).map(String::as_str))?;
        let index = (req.f64_or("page", 1.0) as u32).clamp(1, reader.page_count().max(1)) - 1;
        let (w, h) = reader.page_size(index)?;
        let png = reader.render_png(index, 96.0 * max / w.max(h).max(1.0), false)?;
        crate::common::preview::write_single(req, &png, "png").map(Some)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let in_bytes = file_size(f.input_path());
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let mut doc = pdf::open_file(f, req)?;
        let form: Map<String, Value> = f.item.get("form").and_then(Value::as_object).cloned().unwrap_or_default();
        let edits: Vec<Value> = f.item.get("edits").and_then(Value::as_array).cloned().unwrap_or_default();
        let flatten = req.bool_or("flatten", false);
        if form.is_empty() && edits.is_empty() && !flatten {
            return Err(FileError::new("nothing_to_do"));
        }
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.2, "write", 2, 2);

        // One font for every letter written: field values and text placed on pages.
        let mut texts: Vec<String> = form.values().filter_map(|v| v.as_str().map(String::from)).collect();
        texts.extend(edits.iter().filter(|e| e["type"] == "text").filter_map(|e| e["text"].as_str().map(String::from)));
        let mut font = if texts.iter().any(|t| !t.trim().is_empty()) {
            Some(PdfFont::for_texts(&texts.iter().map(String::as_str).collect::<Vec<_>>())?)
        } else {
            None
        };
        let font_id = doc.new_object_id();

        let filled = fill_form(&mut doc, &form, font.as_mut(), font_id);
        place_edits(&mut doc, &edits, font.as_mut(), font_id, req)?;
        if flatten {
            flatten_form(&mut doc)?;
        } else if filled {
            // Our appearances are what readers show; no reader should redraw them its way.
            // A form that also carries an XFA copy (Adobe LiveCycle) loses it: Acrobat would
            // show the XFA one, without what was filled here.
            with_acroform(&mut doc, |d| {
                d.remove(b"NeedAppearances");
                d.remove(b"XFA");
            });
        }
        if let Some(font) = &font {
            let dict = font.embed(&mut doc)?;
            doc.objects.insert(font_id, Object::Dictionary(dict));
        }
        let out = ctx.file_dir(&f.id)?.join("result.pdf");
        pdf::save(&mut doc, &out, false)?;
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        ctx.emit_output(&f.id, &out, "pdf", json!({}), in_bytes);
        Ok(Value::Null)
    }
}

// ---------- form fields ----------

fn dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    doc.dereference(o).ok()?.1.as_dict().ok()
}

/// A key looked up on the field and then its parents (FT, Ff, V, DA, Q are inherited).
fn inherited(doc: &Document, id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut cur = id;
    for _ in 0..32 {
        let d = doc.get_dictionary(cur).ok()?;
        if let Ok(v) = d.get(key) {
            return doc.dereference(v).ok().map(|(_, o)| o.clone());
        }
        cur = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Changes the form's dictionary, wherever it is kept (its own object or inside the catalog).
fn with_acroform(doc: &mut Document, f: impl FnOnce(&mut Dictionary)) {
    if let Some(id) = acroform_id(doc) {
        if let Ok(d) = doc.get_dictionary_mut(id) {
            f(d);
        }
        return;
    }
    let Ok(root) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) else { return };
    if let Ok(Object::Dictionary(d)) = doc.get_dictionary_mut(root).and_then(|c| c.get_mut(b"AcroForm")) {
        f(d);
    }
}

fn acroform_id(doc: &Document) -> Option<ObjectId> {
    let cat = doc.catalog().ok()?;
    match cat.get(b"AcroForm").ok()? {
        Object::Reference(id) => Some(*id),
        _ => None,
    }
}

/// The field a widget belongs to: itself when it carries a name, else its parent.
fn field_of(doc: &Document, widget: ObjectId) -> ObjectId {
    let d = doc.get_dictionary(widget).ok();
    match d {
        Some(d) if !d.has(b"T") => d.get(b"Parent").ok().and_then(|p| p.as_reference().ok()).unwrap_or(widget),
        _ => widget,
    }
}

/// "parent.child.name", as the form names the field.
fn full_name(doc: &Document, field: ObjectId) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(field);
    for _ in 0..32 {
        let Some(id) = cur else { break };
        let Ok(d) = doc.get_dictionary(id) else { break };
        if let Ok(Object::String(b, _)) = d.get(b"T").and_then(|o| doc.dereference(o).map(|(_, o)| o.clone())) {
            parts.push(decode_text(&b));
        }
        cur = d.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    parts.reverse();
    parts.join(".")
}

/// A PDF text string: UTF-16 with its byte order mark, or the PDF's own 8-bit encoding.
fn decode_text(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let u: Vec<u16> = b[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&u);
    }
    if b.len() >= 3 && b[..3] == [0xEF, 0xBB, 0xBF] {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    b.iter().map(|c| *c as char).collect()
}

fn name_or_text(o: &Object) -> Option<String> {
    match o {
        Object::String(b, _) => Some(decode_text(b)),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

/// The states a check box or radio button can show, other than Off.
fn on_states(doc: &Document, widget: ObjectId) -> Vec<String> {
    let Some(ap) = doc.get_dictionary(widget).ok().and_then(|d| d.get(b"AP").ok()).and_then(|o| dict(doc, o)) else { return vec![] };
    let Some(n) = ap.get(b"N").ok().and_then(|o| dict(doc, o)) else { return vec![] };
    n.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).filter(|k| k != "Off").collect()
}

struct Widget {
    id: ObjectId,
    page: usize,
    page_id: ObjectId,
    rect: [f64; 4],
}

/// Every widget on the pages, in page order.
fn widgets(doc: &Document) -> Vec<Widget> {
    let mut out = Vec::new();
    for (i, (_, page_id)) in doc.get_pages().into_iter().enumerate() {
        let Ok(page) = doc.get_dictionary(page_id) else { continue };
        let Some(annots) = page.get(b"Annots").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok().cloned()) else { continue };
        for a in annots {
            let Ok(id) = a.as_reference() else { continue };
            let Ok(d) = doc.get_dictionary(id) else { continue };
            if d.get(b"Subtype").and_then(|o| o.as_name()).ok() != Some(b"Widget".as_slice()) {
                continue;
            }
            let r: Vec<f64> = d.get(b"Rect").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok())
                .map(|a| a.iter().filter_map(|o| pdf::num(doc, o)).collect()).unwrap_or_default();
            if r.len() != 4 {
                continue;
            }
            out.push(Widget { id, page: i + 1, page_id, rect: [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])] });
        }
    }
    out
}

/// A rectangle in the page's own space → [x, y, w, h] as fractions of the page as shown,
/// from its top-left corner.
fn to_shown(doc: &Document, page: ObjectId, r: [f64; 4]) -> [f64; 4] {
    let (bx0, by0, bx1, by1) = pdf::visible_box(doc, page);
    let (ox, oy) = (bx0.min(bx1), by0.min(by1));
    let (w, h) = ((bx1 - bx0).abs(), (by1 - by0).abs());
    let (sw, sh) = pdf::displayed_size(doc, page);
    // A point in page space → shown space (origin bottom-left).
    let map = |px: f64, py: f64| {
        let (x, y) = (px - ox, py - oy);
        match pdf::rotation(doc, page) {
            90 => (y, w - x),
            180 => (w - x, h - y),
            270 => (h - y, x),
            _ => (x, y),
        }
    };
    let (ax, ay) = map(r[0], r[1]);
    let (bx, by) = map(r[2], r[3]);
    let (x0, x1, y0, y1) = (ax.min(bx), ax.max(bx), ay.min(by), ay.max(by));
    let round = |v: f64| (v * 100000.0).round() / 100000.0;
    [round(x0 / sw), round(1.0 - y1 / sh), round((x1 - x0) / sw), round((y1 - y0) / sh)]
}

/// The form's fields for the editor, one entry per widget.
fn fields(doc: &Document) -> Vec<Value> {
    let mut out = Vec::new();
    for w in widgets(doc) {
        let field = field_of(doc, w.id);
        let name = full_name(doc, field);
        if name.is_empty() {
            continue;
        }
        let ft = inherited(doc, w.id, b"FT").and_then(|o| o.as_name().ok().map(|n| n.to_vec())).unwrap_or_default();
        let ff = inherited(doc, w.id, b"Ff").and_then(|o| o.as_i64().ok()).unwrap_or(0);
        let value = inherited(doc, field, b"V").and_then(|o| name_or_text(&o));
        let kind = match ft.as_slice() {
            b"Tx" => "text",
            b"Ch" => "choice",
            b"Sig" => "signature",
            b"Btn" if ff & (1 << 16) != 0 => continue, // push buttons: nothing to fill
            b"Btn" if ff & (1 << 15) != 0 => "radio",
            b"Btn" => "check",
            _ => continue,
        };
        let mut e = json!({ "name": name, "type": kind, "page": w.page, "rect": to_shown(doc, w.page_id, w.rect),
                            "readOnly": ff & 1 != 0 });
        if let Some(v) = value {
            e["value"] = json!(v);
        }
        match kind {
            "text" => {
                e["multiline"] = json!(ff & (1 << 12) != 0);
                e["password"] = json!(ff & (1 << 13) != 0);
                if let Some(n) = inherited(doc, field, b"MaxLen").and_then(|o| o.as_i64().ok()) {
                    e["maxLen"] = json!(n);
                }
            }
            "check" | "radio" => {
                e["on"] = json!(on_states(doc, w.id).into_iter().next().unwrap_or_else(|| "Yes".into()));
            }
            "choice" => {
                let opts: Vec<Value> = inherited(doc, field, b"Opt").and_then(|o| o.as_array().ok().cloned()).unwrap_or_default().iter()
                    .filter_map(|o| match doc.dereference(o).ok()?.1 {
                        Object::Array(pair) if pair.len() == 2 => Some(json!([name_or_text(&pair[0])?, name_or_text(&pair[1])?])),
                        other => name_or_text(other).map(|s| json!([s, s])),
                    })
                    .collect();
                e["options"] = json!(opts);
                e["combo"] = json!(ff & (1 << 17) != 0);
            }
            _ => {}
        }
        out.push(e);
    }
    out
}

/// "/Helv 10 Tf 0 0 1 rg" → (10, "0 0 1 rg").
fn parse_da(da: &str) -> (f64, String) {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut size = 0.0;
    let mut color = "0 g".to_string();
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "Tf" if i >= 1 => size = toks[i - 1].parse().unwrap_or(0.0),
            "g" if i >= 1 => color = format!("{} g", toks[i - 1]),
            "rg" if i >= 3 => color = format!("{} {} {} rg", toks[i - 3], toks[i - 2], toks[i - 1]),
            "k" if i >= 4 => color = format!("{} {} {} {} k", toks[i - 4], toks[i - 3], toks[i - 2], toks[i - 1]),
            _ => {}
        }
    }
    (size, color)
}

/// Words laid out in lines no wider than `width` (at size 1 → `width` / size).
fn wrap(font: &mut PdfFont, text: &str, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && font.shape(&candidate).width > width {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

/// The look of a text field holding `text`, drawn with our font (any language).
#[allow(clippy::too_many_arguments)]
fn text_appearance(doc: &mut Document, font: &mut PdfFont, font_id: ObjectId, w: f64, h: f64, text: &str, da: &str, quad: i64, multiline: bool) -> ObjectId {
    let (da_size, color) = parse_da(da);
    let pad = 2.0;
    let mut s = format!("/Tx BMC q {pad} {pad} {:.2} {:.2} re W n BT /FAP ", (w - 2.0 * pad).max(0.0), (h - 2.0 * pad).max(0.0));
    if multiline {
        let size = if da_size > 0.0 { da_size } else { 10.0 };
        let lines = wrap(font, text, (w - 2.0 * pad) / size);
        let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{size:.2} Tf {color}\n"));
        for (i, l) in lines.iter().enumerate() {
            let run = font.shape(l);
            let x = match quad { 1 => (w - run.width * size) / 2.0, 2 => w - pad - run.width * size, _ => pad };
            let y = h - pad - size * (i as f64 + 1.0) * 1.15 + size * 0.2;
            let _ = std::fmt::Write::write_fmt(&mut s, format_args!("1 0 0 1 {x:.2} {y:.2} Tm {}\n", run.op()));
        }
    } else {
        let line = text.replace('\n', " ");
        let run = font.shape(&line);
        let mut size = if da_size > 0.0 { da_size } else { (h * 0.62).min(12.0) };
        // Auto size, or a value too wide for the box: smaller until it fits.
        if run.width * size > w - 2.0 * pad && (da_size == 0.0 || run.width > 0.0) {
            size = size.min((w - 2.0 * pad) / run.width.max(0.01)).max(4.0);
        }
        let x = match quad { 1 => (w - run.width * size) / 2.0, 2 => w - pad - run.width * size, _ => pad };
        let y = (h - size) / 2.0 + size * 0.22;
        let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{size:.2} Tf {color} 1 0 0 1 {x:.2} {y:.2} Tm {}\n", run.op()));
    }
    s.push_str("ET Q EMC\n");
    let stream = Stream::new(dictionary! {
        "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), Object::Real(w as f32), Object::Real(h as f32)],
        "Resources" => dictionary! { "Font" => dictionary! { "FAP" => font_id } }
    }, s.into_bytes());
    doc.add_object(stream)
}

/// Writes the values into the fields. Returns whether anything was filled.
fn fill_form(doc: &mut Document, form: &Map<String, Value>, mut font: Option<&mut PdfFont>, font_id: ObjectId) -> bool {
    if form.is_empty() {
        return false;
    }
    let mut by_field: HashMap<ObjectId, Vec<Widget>> = HashMap::new();
    let mut names: HashMap<String, ObjectId> = HashMap::new();
    for w in widgets(doc) {
        let field = field_of(doc, w.id);
        names.entry(full_name(doc, field)).or_insert(field);
        by_field.entry(field).or_default().push(w);
    }
    let mut filled = false;
    for (name, value) in form {
        let Some(&field) = names.get(name) else { continue };
        let Some(ws) = by_field.get(&field) else { continue };
        let ft = inherited(doc, field, b"FT").and_then(|o| o.as_name().ok().map(|n| n.to_vec())).unwrap_or_default();
        let ff = inherited(doc, field, b"Ff").and_then(|o| o.as_i64().ok()).unwrap_or(0);
        if ff & 1 != 0 {
            continue; // read-only
        }
        match ft.as_slice() {
            b"Btn" => {
                // A check box: true/false. A radio group: the state chosen.
                let chosen: Option<String> = match value {
                    Value::Bool(true) => ws.first().and_then(|w| on_states(doc, w.id).into_iter().next()).or(Some("Yes".into())),
                    Value::String(s) if !s.is_empty() && s != "Off" => Some(s.clone()),
                    _ => None,
                };
                let v = Object::Name(chosen.clone().unwrap_or_else(|| "Off".into()).into_bytes());
                if let Ok(d) = doc.get_dictionary_mut(field) {
                    d.set("V", v);
                }
                for w in ws {
                    let states = on_states(doc, w.id);
                    let as_ = match &chosen {
                        Some(c) if states.contains(c) => c.clone(),
                        _ => "Off".into(),
                    };
                    if let Ok(d) = doc.get_dictionary_mut(w.id) {
                        d.set("AS", Object::Name(as_.into_bytes()));
                    }
                }
                filled = true;
            }
            b"Tx" | b"Ch" => {
                let text = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                if let Ok(d) = doc.get_dictionary_mut(field) {
                    d.set("V", pdf::text_string(&text));
                }
                let Some(font) = font.as_deref_mut() else {
                    filled = true;
                    continue;
                };
                let da = inherited(doc, field, b"DA").and_then(|o| name_or_text(&o))
                    .or_else(|| acroform_id(doc).and_then(|a| inherited(doc, a, b"DA")).and_then(|o| name_or_text(&o)))
                    .unwrap_or_default();
                let quad = inherited(doc, field, b"Q").and_then(|o| o.as_i64().ok()).unwrap_or(0);
                let multiline = ft.as_slice() == b"Tx" && ff & (1 << 12) != 0;
                for w in ws {
                    let (rw, rh) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
                    let ap = text_appearance(doc, font, font_id, rw, rh, &text, &da, quad, multiline);
                    if let Ok(d) = doc.get_dictionary_mut(w.id) {
                        d.set("AP", dictionary! { "N" => ap });
                    }
                }
                filled = true;
            }
            _ => {}
        }
    }
    filled
}

/// Every widget drawn into its page as it looks now; then the form is removed.
fn flatten_form(doc: &mut Document) -> Result<(), FileError> {
    let ws = widgets(doc);
    let mut per_page: HashMap<ObjectId, (String, Vec<(String, Object)>)> = HashMap::new();
    let mut removed: Vec<ObjectId> = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        removed.push(w.id);
        let Ok(d) = doc.get_dictionary(w.id) else { continue };
        let flags = d.get(b"F").and_then(|o| o.as_i64()).unwrap_or(0);
        if flags & 2 != 0 {
            continue; // hidden
        }
        let Some(n) = d.get(b"AP").ok().and_then(|o| dict(doc, o)).and_then(|ap| ap.get(b"N").ok()).cloned() else { continue };
        let stream_id = match doc.dereference(&n).ok().map(|(id, o)| (id, o.clone())) {
            Some((Some(id), Object::Stream(_))) => id,
            Some((_, Object::Dictionary(states))) => {
                let state = d.get(b"AS").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec()).unwrap_or_default();
                match states.get(&state).ok().and_then(|o| o.as_reference().ok()) {
                    Some(id) => id,
                    None => continue,
                }
            }
            _ => continue,
        };
        let Ok(stream) = doc.get_object(stream_id).and_then(|o| o.as_stream()) else { continue };
        let bbox: Vec<f64> = stream.dict.get(b"BBox").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok())
            .map(|a| a.iter().filter_map(|o| pdf::num(doc, o)).collect()).unwrap_or_default();
        if bbox.len() != 4 {
            continue;
        }
        let (bw, bh) = ((bbox[2] - bbox[0]).abs().max(0.001), (bbox[3] - bbox[1]).abs().max(0.001));
        let (sx, sy) = ((w.rect[2] - w.rect[0]) / bw, (w.rect[3] - w.rect[1]) / bh);
        let (tx, ty) = (w.rect[0] - bbox[0].min(bbox[2]) * sx, w.rect[1] - bbox[1].min(bbox[3]) * sy);
        if let Ok(Object::Stream(s)) = doc.get_object_mut(stream_id) {
            s.dict.set("Type", "XObject");
            s.dict.set("Subtype", "Form");
        }
        let entry = per_page.entry(w.page_id).or_insert_with(|| (String::new(), Vec::new()));
        let name = format!("MQF{i}");
        entry.0.push_str(&format!("q {sx:.5} 0 0 {sy:.5} {tx:.3} {ty:.3} cm /{name} Do Q\n"));
        entry.1.push((name, Object::Reference(stream_id)));
    }
    for (page, (content, xobjects)) in per_page {
        let res: Vec<(&str, &str, Object)> = xobjects.iter().map(|(n, o)| ("XObject", n.as_str(), o.clone())).collect();
        pdf::append_content(doc, page, content.into_bytes(), &res)?;
    }
    // The widgets leave the pages, and the form leaves the document.
    for (_, page_id) in doc.get_pages() {
        let annots = doc.get_dictionary(page_id).ok().and_then(|d| d.get(b"Annots").ok()).and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok().cloned());
        if let Some(a) = annots {
            let kept: Vec<Object> = a.into_iter().filter(|o| o.as_reference().map(|id| !removed.contains(&id)).unwrap_or(true)).collect();
            if let Ok(d) = doc.get_dictionary_mut(page_id) {
                if kept.is_empty() { d.remove(b"Annots"); } else { d.set("Annots", kept); }
            }
        }
    }
    if let Ok(root) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
        if let Ok(cat) = doc.get_dictionary_mut(root) {
            cat.remove(b"AcroForm");
        }
    }
    Ok(())
}

// ---------- things placed on the pages ----------

fn rgb(c: &str) -> String {
    let c = img::parse_color(c);
    format!("{:.3} {:.3} {:.3}", c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0)
}

fn place_edits(doc: &mut Document, edits: &[Value], mut font: Option<&mut PdfFont>, font_id: ObjectId, req: &Request) -> Result<(), FileError> {
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let mut per_page: HashMap<usize, (String, Vec<(String, Object)>)> = HashMap::new();
    let highlight = doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => 0.4, "BM" => "Multiply" });
    for (i, e) in edits.iter().enumerate() {
        let page_no = e["page"].as_u64().unwrap_or(1) as usize;
        let Some(&page) = pages.get(page_no.saturating_sub(1)) else { continue };
        let (pw, ph) = pdf::displayed_size(doc, page);
        let f = |k: &str| e[k].as_f64().unwrap_or(0.0);
        // Shown space, origin bottom-left, in points.
        let (x, w, h) = (f("x") * pw, f("w") * pw, f("h") * ph);
        let y = ph - f("y") * ph - h;
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let color = rgb(e["color"].as_str().unwrap_or("#000000"));
        let entry = per_page.entry(page_no).or_insert_with(|| (format!("q {}\n", pdf::display_cm(doc, page)), Vec::new()));
        match e["type"].as_str().unwrap_or("") {
            "text" => {
                let Some(font) = font.as_deref_mut() else { continue };
                let text = e["text"].as_str().unwrap_or("");
                let size = (f("size") * ph).max(4.0);
                entry.0.push_str(&format!("BT /FED {size:.2} Tf {color} rg\n"));
                for (j, line) in text.split('\n').enumerate() {
                    let run = font.shape(line);
                    let ly = y + h - size * 0.92 - j as f64 * size * 1.25;
                    entry.0.push_str(&format!("1 0 0 1 {x:.2} {ly:.2} Tm {}\n", run.op()));
                }
                entry.0.push_str("ET\n");
                if !entry.1.iter().any(|(n, _)| n == "FED") {
                    entry.1.push(("FED".into(), Object::Reference(font_id)));
                }
            }
            "image" | "signature" => {
                let src = e["src"].as_str().unwrap_or("");
                let im = match b64::data_url(src) {
                    Some(bytes) => image::load_from_memory(&bytes).map_err(|_| FileError::new("bad_picture"))?,
                    None if !src.is_empty() => img::load(std::path::Path::new(src), req).map_err(|_| FileError::new("bad_picture"))?.img,
                    None => continue,
                };
                // Pictures larger than needed for 300 dpi at their placed size are made smaller.
                let max_px = ((w.max(h) / 72.0) * 300.0).ceil().max(64.0) as u32;
                let im = if im.width().max(im.height()) > max_px { im.resize(max_px, max_px, image::imageops::FilterType::Lanczos3) } else { im };
                let (id, ..) = pdf::image_xobject(doc, &im);
                let name = format!("IED{i}");
                entry.0.push_str(&format!("q {w:.3} 0 0 {h:.3} {x:.3} {y:.3} cm /{name} Do Q\n"));
                entry.1.push((name, Object::Reference(id)));
            }
            "check" => {
                let lw = w.min(h) * 0.12;
                entry.0.push_str(&format!("q {color} RG {lw:.2} w 1 J 1 j {:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l S Q\n",
                    x + w * 0.15, y + h * 0.5, x + w * 0.4, y + h * 0.2, x + w * 0.85, y + h * 0.82));
            }
            "cross" => {
                let lw = w.min(h) * 0.12;
                entry.0.push_str(&format!("q {color} RG {lw:.2} w 1 J {:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} m {:.2} {:.2} l S Q\n",
                    x + w * 0.18, y + h * 0.18, x + w * 0.82, y + h * 0.82, x + w * 0.18, y + h * 0.82, x + w * 0.82, y + h * 0.18));
            }
            "whiteout" => {
                let c = rgb(e["color"].as_str().unwrap_or("#ffffff"));
                entry.0.push_str(&format!("q {c} rg {x:.2} {y:.2} {w:.2} {h:.2} re f Q\n"));
            }
            "highlight" => {
                let c = rgb(e["color"].as_str().unwrap_or("#ffe14d"));
                entry.0.push_str(&format!("q /GSHL gs {c} rg {x:.2} {y:.2} {w:.2} {h:.2} re f Q\n"));
                if !entry.1.iter().any(|(n, _)| n == "GSHL") {
                    entry.1.push(("GSHL".into(), Object::Reference(highlight)));
                }
            }
            _ => {}
        }
    }
    for (page_no, (mut content, objects)) in per_page {
        content.push_str("Q\n");
        let res: Vec<(&str, &str, Object)> = objects.iter().map(|(n, o)| {
            let cat = if n == "FED" { "Font" } else if n == "GSHL" { "ExtGState" } else { "XObject" };
            (cat, n.as_str(), o.clone())
        }).collect();
        pdf::append_content(doc, pages[page_no - 1], content.into_bytes(), &res)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with a text field, a check box and a radio pair, saved to `path`.
    fn form_pdf(path: &std::path::Path, rotate: i64) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let page_id = doc.new_object_id();
        let on = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] },
                                            b"0 0 0 rg 2 2 8 8 re f".to_vec()));
        let off = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] }, vec![]));
        let name = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("name"),
            "Rect" => vec![100.into(), 700.into(), 300.into(), 720.into()], "DA" => Object::string_literal("/Helv 0 Tf 0 g"), "P" => page_id });
        let agree = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => Object::string_literal("agree"),
            "Rect" => vec![100.into(), 650.into(), 112.into(), 662.into()], "AS" => "Off",
            "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } } });
        let group = doc.new_object_id();
        let r1 = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => group,
            "Rect" => vec![100.into(), 600.into(), 112.into(), 612.into()], "AS" => "Off", "AP" => dictionary! { "N" => dictionary! { "a" => on, "Off" => off } } });
        let r2 = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => group,
            "Rect" => vec![150.into(), 600.into(), 162.into(), 612.into()], "AS" => "Off", "AP" => dictionary! { "N" => dictionary! { "b" => on, "Off" => off } } });
        doc.objects.insert(group, Object::Dictionary(dictionary! { "FT" => "Btn", "Ff" => 1i64 << 15, "T" => Object::string_literal("pick"), "Kids" => vec![r1.into(), r2.into()] }));
        let content = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        doc.objects.insert(page_id, Object::Dictionary(dictionary! { "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Rotate" => rotate, "Contents" => content, "Annots" => vec![name.into(), agree.into(), r1.into(), r2.into()] }));
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
        let acro = doc.add_object(dictionary! { "Fields" => vec![name.into(), agree.into(), group.into()], "NeedAppearances" => true });
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => acro });
        doc.trailer.set("Root", cat);
        doc.save(path).unwrap();
    }

    #[test]
    fn reads_fills_and_flattens() {
        let dir = std::env::temp_dir().join(format!("mqkit-pdf-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("form.pdf");
        form_pdf(&src, 0);
        let doc = pdf::open(&src, None).unwrap();
        let fs = fields(&doc);
        let names: Vec<&str> = fs.iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["name", "agree", "pick", "pick"]);
        assert_eq!(fs[0]["type"], "text");
        assert_eq!(fs[1]["on"], "Yes");
        assert_eq!(fs[3]["on"], "b");
        // 100..300 × 700..720 on a 612 × 792 page, from the top-left.
        let r: Vec<f64> = fs[0]["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((r[0] - 100.0 / 612.0).abs() < 1e-4 && (r[1] - 72.0 / 792.0).abs() < 1e-4 && (r[2] - 200.0 / 612.0).abs() < 1e-4);

        let mut doc = pdf::open(&src, None).unwrap();
        let form: Map<String, Value> = serde_json::from_value(json!({ "name": "Ana Pop — Ștefănescu 東京", "agree": true, "pick": "b" })).unwrap();
        let mut font = PdfFont::for_texts(&["Ana Pop — Ștefănescu 東京", "Semnat"]).unwrap();
        let font_id = doc.new_object_id();
        assert!(fill_form(&mut doc, &form, Some(&mut font), font_id));
        let edits = vec![
            json!({ "type": "text", "page": 1, "x": 0.1, "y": 0.5, "w": 0.3, "h": 0.05, "size": 0.02, "text": "Semnat", "color": "#1a3cc8" }),
            json!({ "type": "check", "page": 1, "x": 0.5, "y": 0.5, "w": 0.03, "h": 0.03 }),
            json!({ "type": "highlight", "page": 1, "x": 0.1, "y": 0.6, "w": 0.3, "h": 0.02 }),
            json!({ "type": "signature", "page": 1, "x": 0.6, "y": 0.8, "w": 0.2, "h": 0.06,
                    "src": format!("data:image/png;base64,{}", b64::encode(&img::encode_png(&image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(40, 12, image::Rgba([0, 0, 120, 200]))), false, 100).unwrap())) }),
        ];
        let req: Request = serde_json::from_value(json!({ "cmd": "run" })).unwrap();
        place_edits(&mut doc, &edits, Some(&mut font), font_id, &req).unwrap();
        let dict = font.embed(&mut doc).unwrap();
        doc.objects.insert(font_id, Object::Dictionary(dict));
        let filled = dir.join("filled.pdf");
        pdf::save(&mut doc, &filled, false).unwrap();

        // The values are in the fields, and the radio shows the second button.
        let doc = pdf::open(&filled, None).unwrap();
        let fs = fields(&doc);
        assert_eq!(fs[0]["value"], "Ana Pop — Ștefănescu 東京");
        assert_eq!(fs[1]["value"], "Yes");
        assert_eq!(fs[2]["value"], "b");

        // Flattened: no form left, the page draws the field appearances.
        let mut doc = pdf::open(&filled, None).unwrap();
        flatten_form(&mut doc).unwrap();
        let out = dir.join("flat.pdf");
        pdf::save(&mut doc, &out, false).unwrap();
        let doc = pdf::open(&out, None).unwrap();
        assert!(fields(&doc).is_empty());
        assert!(!doc.catalog().unwrap().has(b"AcroForm"));
        let page = *doc.get_pages().values().next().unwrap();
        let content = String::from_utf8_lossy(&doc.get_page_content(page)).to_string();
        assert!(content.contains("/MQF0 Do") && content.contains("/IED3 Do"), "{content}");
        // Windows reads the result.
        crate::common::winrt::Pdf::open(&out, None).unwrap().render_png(0, 50.0, false).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotated_pages_map_to_what_is_shown() {
        let dir = std::env::temp_dir().join(format!("mqkit-pdf-edit-rot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("rot.pdf");
        form_pdf(&src, 90);
        let doc = pdf::open(&src, None).unwrap();
        // Turned right (clockwise): the page shows 792 wide, and its top edge is now on the
        // right; the field (x 100..300, y 700..720) shows 700..720 pt from the left, 100 pt
        // from the top, standing upright.
        let r: Vec<f64> = fields(&doc)[0]["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((r[0] - 700.0 / 792.0).abs() < 1e-4, "{r:?}");
        assert!((r[1] - 100.0 / 612.0).abs() < 1e-4, "{r:?}");
        assert!((r[2] - 20.0 / 792.0).abs() < 1e-4 && (r[3] - 200.0 / 612.0).abs() < 1e-4, "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn da_and_text_strings() {
        assert_eq!(parse_da("/Helv 10 Tf 0 0 1 rg"), (10.0, "0 0 1 rg".to_string()));
        assert_eq!(parse_da("/F1 0 Tf 0.5 g").0, 0.0);
        assert_eq!(decode_text(&[0xFE, 0xFF, 0x00, 0x41, 0x01, 0x03]), "Aă");
        assert_eq!(decode_text(b"plain"), "plain");
    }
}
