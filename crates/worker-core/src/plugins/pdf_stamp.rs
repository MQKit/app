//! Watermark (text or picture) and page numbers on PDF pages. Text in any language is
//! drawn with a Windows font that has its letters, embedding only the glyphs used.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{font::PdfFont, img, pdf};
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use lopdf::{dictionary, Document, Object, ObjectId};
use serde_json::{json, Value};

pub struct PdfStamp;

impl Plugin for PdfStamp {
    fn id(&self) -> &'static str {
        "pdf-stamp"
    }

    fn manifest(&self) -> Value {
        let mark = |k: &str| json!({ "field": "kind", "eq": k });
        let text_mark = json!({ "all": [ mark("watermark"), { "field": "markType", "eq": "text" } ] });
        json!({
            "name": t("Filigran și numerotare pagini", "Watermark and page numbers"),
            "summary": t("Marchează fiecare pagină", "Mark every page"),
            "keywords": { "ro": ["filigran", "numerotare", "numere de pagină", "ștampilă", "confidențial"], "en": ["watermark", "page numbers", "number pages", "stamp", "confidential"] },
            "category": "pdf",
            "icon": "rubber-stamp",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"], "replaceOriginalAllowed": true },
            "analysis": { "fast": ["needsPassword", "pageCount", "isSigned"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.1 },
            "presets": [
                { "id": "numbers", "label": t("Numere jos, la mijloc", "Numbers at the bottom"), "default": true,
                  "values": { "kind": "numbers", "numberFormat": "n", "position": "bottomCenter" } },
                { "id": "ofTotal", "label": t("„Pagina 1 din 9”", "“Page 1 of 9”"), "values": { "kind": "numbers", "numberFormat": "pageOf", "position": "bottomRight" } },
                { "id": "confidential", "label": t("CONFIDENȚIAL pe diagonală", "CONFIDENTIAL diagonally"),
                  "values": { "kind": "watermark", "markType": "text", "text": "CONFIDENȚIAL", "layout": "diagonal", "opacity": 20 } },
                { "id": "draft", "label": t("Ciornă", "Draft"), "values": { "kind": "watermark", "markType": "text", "text": "CIORNĂ", "layout": "diagonal", "opacity": 15 } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "kind", "type": "select", "label": t("Ce adaugi", "What to add"), "default": "numbers", "display": "chips",
                  "options": [ opt("numbers", "Numere de pagină", "Page numbers"), opt("watermark", "Filigran", "Watermark") ] },
                { "key": "numberFormat", "type": "select", "label": t("Cum arată", "Looks like"), "default": "n", "showIf": mark("numbers"),
                  "options": [ opt("n", "1", "1"), opt("nOfTotal", "1 / 9", "1 / 9"), opt("pageOf", "Pagina 1 din 9", "Page 1 of 9"), opt("dash", "– 1 –", "– 1 –") ] },
                { "key": "position", "type": "select", "label": t("Unde", "Where"), "default": "bottomCenter", "showIf": mark("numbers"),
                  "options": [ opt("bottomCenter", "Jos, la mijloc", "Bottom center"), opt("bottomRight", "Jos, dreapta", "Bottom right"),
                               opt("bottomLeft", "Jos, stânga", "Bottom left"), opt("topCenter", "Sus, la mijloc", "Top center"),
                               opt("topRight", "Sus, dreapta", "Top right"), opt("outside", "Jos, la exterior (carte)", "Bottom outside (book)") ] },
                { "key": "startAt", "type": "number", "label": t("Începe de la", "Start at"), "default": 1, "min": 0, "max": 99999, "showIf": mark("numbers") },
                { "key": "markType", "type": "select", "label": t("Filigran din", "Watermark from"), "default": "text", "display": "chips", "showIf": mark("watermark"),
                  "options": [ opt("text", "Text", "Text"), opt("image", "Imagine", "Image") ] },
                { "key": "text", "type": "text", "label": t("Textul", "The text"), "default": "", "placeholder": t("CONFIDENȚIAL", "CONFIDENTIAL"), "showIf": text_mark },
                { "key": "image", "type": "file", "label": t("Imaginea (PNG cu transparență e ideal)", "The image (PNG with transparency is best)"),
                  "accept": ["png", "jpg", "jpeg"], "showIf": { "all": [ mark("watermark"), { "field": "markType", "eq": "image" } ] } },
                { "key": "layout", "type": "select", "label": t("Așezare", "Layout"), "default": "diagonal", "showIf": mark("watermark"),
                  "options": [ opt("diagonal", "Pe diagonală, la mijloc", "Diagonal, in the middle"), opt("center", "Drept, la mijloc", "Straight, in the middle"),
                               opt("tiled", "Repetat pe toată pagina", "Repeated over the page"), opt("bottomRight", "Mic, jos în dreapta", "Small, bottom right") ] },
                { "key": "opacity", "type": "range", "label": t("Vizibilitate", "Visibility"), "min": 5, "max": 100, "step": 5, "unit": "%", "default": 20, "showIf": mark("watermark") },
                { "key": "color", "type": "color", "label": t("Culoare", "Color"), "default": "#c0392b", "showIf": { "any": [ text_mark, mark("numbers") ] } },
                { "key": "fontSize", "type": "range", "advanced": true, "group": "more", "min": 6, "max": 120, "step": 1, "default": 11,
                  "label": t("Mărimea textului (numere)", "Text size (numbers)"), "showIf": mark("numbers") },
                { "key": "pages", "type": "pages", "syntax": "set", "advanced": true, "group": "more", "label": t("Pe ce pagini", "On which pages"), "default": "all",
                  "placeholder": t("toate", "all"), "help": t("De exemplu 2-z (fără prima pagină).", "For example 2-z (not the first page).") }
            ],
            "rules": [
                { "id": "signature_lost", "when": { "file": "isSigned", "eq": true }, "severity": "confirm",
                  "message": t("Semnăturile digitale nu vor mai fi valide.", "Digital signatures won't be valid anymore.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_marcat", "{name}_marked") } },
            "errors": { "no_image": t("Alege imaginea pentru filigran.", "Choose the image for the watermark.") }
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
        let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        let total = ids.len() as u32;
        let chosen = pages::parse_set(req.str_or("pages", "all"), total)
            .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": bad, "pageCount": total })))?;
        let words = crate::common::words::of(req.locale.as_deref());
        let kind = req.str_or("kind", "numbers");
        let image = if kind == "watermark" && req.str_or("markType", "text") == "image" { Some(load_mark(&mut doc, req)?) } else { None };
        let start = req.f64_or("startAt", 1.0) as i64;
        let label = |p: u32| {
            let num = start + p as i64 - 1;
            let last = start + total as i64 - 1;
            match req.str_or("numberFormat", "n") {
                "nOfTotal" => format!("{num} / {last}"),
                "pageOf" => words.page_of(num, last),
                "dash" => format!("– {num} –"),
                _ => num.to_string(),
            }
        };
        let text = match req.str_or("text", "").trim() {
            "" => words.confidential.to_string(),
            t => t.to_string(),
        };
        // One font with every letter needed; its object is filled in at the end.
        let labels: Vec<String> = if kind == "numbers" { chosen.iter().map(|p| label(*p)).collect() } else { vec![text.clone()] };
        let mut font = if kind == "numbers" || image.is_none() {
            Some(PdfFont::for_texts(&labels.iter().map(String::as_str).collect::<Vec<_>>())?)
        } else {
            None
        };
        let font_id = doc.new_object_id();
        let opacity = (req.f64_or("opacity", 20.0) / 100.0).clamp(0.05, 1.0);
        let gs = doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => opacity as f32, "CA" => opacity as f32 });
        let color = img::parse_color(req.str_or("color", "#c0392b"));
        let rgb = format!("{:.3} {:.3} {:.3}", color[0] as f64 / 255.0, color[1] as f64 / 255.0, color[2] as f64 / 255.0);
        let n = chosen.len().max(1);
        for (i, p) in chosen.iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, 0.1 + 0.8 * i as f64 / n as f64, "write", 2, 2);
            let page = ids[(*p - 1) as usize];
            let (w, h) = pdf::displayed_size(&doc, page);
            let mut s = format!("q {}\n", pdf::display_cm(&doc, page));
            let mut res: Vec<(&str, &str, Object)> = vec![("ExtGState", "GSTAMP", gs.into())];
            if font.is_some() {
                res.push(("Font", "FSTAMP", font_id.into()));
            }
            if let (Some(font), "numbers") = (font.as_mut(), kind) {
                let run = font.shape(&label(*p));
                let size = req.f64_or("fontSize", 11.0);
                let tw = run.width * size;
                let m = 28.0;
                let pos = match req.str_or("position", "bottomCenter") {
                    "outside" if p % 2 == 0 => "bottomLeft",
                    "outside" => "bottomRight",
                    other => other,
                };
                let (x, y) = match pos {
                    "bottomRight" => (w - m - tw, m * 0.8),
                    "bottomLeft" => (m, m * 0.8),
                    "topCenter" => ((w - tw) / 2.0, h - m - size),
                    "topRight" => (w - m - tw, h - m - size),
                    _ => ((w - tw) / 2.0, m * 0.8),
                };
                s.push_str(&format!("BT /FSTAMP {size:.2} Tf {rgb} rg 1 0 0 1 {x:.2} {y:.2} Tm {} ET\n", run.op()));
            } else if let Some((img_id, iw, ih)) = image {
                res.push(("XObject", "IMSTAMP", img_id.into()));
                let place = |cx: f64, cy: f64, dw: f64| {
                    let dh = dw * ih / iw;
                    format!("q {dw:.2} 0 0 {dh:.2} {:.2} {:.2} cm /IMSTAMP Do Q\n", cx - dw / 2.0, cy - dh / 2.0)
                };
                s.push_str("/GSTAMP gs\n");
                match req.str_or("layout", "diagonal") {
                    "tiled" => {
                        let dw = w / 4.0;
                        let mut y = dw * 0.6;
                        while y < h {
                            let mut x = dw * 0.6;
                            while x < w {
                                s.push_str(&place(x, y, dw * 0.7));
                                x += dw * 1.2;
                            }
                            y += dw * ih / iw * 1.6 + 20.0;
                        }
                    }
                    "bottomRight" => s.push_str(&place(w - w * 0.12 - 20.0, w * 0.12 * ih / iw / 2.0 + 20.0, w * 0.24)),
                    _ => s.push_str(&place(w / 2.0, h / 2.0, w * 0.55)),
                }
            } else if let Some(font) = font.as_mut() {
                let run = font.shape(&text);
                s.push_str(&format!("/GSTAMP gs {rgb} rg\n"));
                let diag = (h.atan2(w)).to_degrees();
                match req.str_or("layout", "diagonal") {
                    "tiled" => {
                        let size = (w / 14.0).max(12.0);
                        let tw = run.width * size;
                        let (c, sn) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
                        let mut y = -h * 0.2;
                        while y < h * 1.1 {
                            let mut x = -w * 0.2;
                            while x < w * 1.1 {
                                s.push_str(&format!("BT /FSTAMP {size:.2} Tf {c:.4} {sn:.4} {:.4} {c:.4} {x:.2} {y:.2} Tm {} ET\n", -sn, run.op()));
                                x += tw + size * 2.0;
                            }
                            y += size * 4.0;
                        }
                    }
                    "bottomRight" => {
                        let size = 14.0;
                        let tw = run.width * size;
                        s.push_str(&format!("BT /FSTAMP {size} Tf 1 0 0 1 {:.2} 24 Tm {} ET\n", w - tw - 28.0, run.op()));
                    }
                    layout => {
                        let angle = if layout == "center" { 0.0 } else { diag };
                        let span = if layout == "center" { w * 0.8 } else { (w * w + h * h).sqrt() * 0.7 };
                        let size = (span / run.width.max(0.1)).min(w * 0.3);
                        let tw = run.width * size;
                        let (c, sn) = (angle.to_radians().cos(), angle.to_radians().sin());
                        // Centered: start half the text back along the angle, and down by a third of the height.
                        let x = w / 2.0 - c * tw / 2.0 + sn * size * 0.35;
                        let y = h / 2.0 - sn * tw / 2.0 - c * size * 0.35;
                        s.push_str(&format!("BT /FSTAMP {size:.2} Tf {c:.4} {sn:.4} {:.4} {c:.4} {x:.2} {y:.2} Tm {} ET\n", -sn, run.op()));
                    }
                }
            }
            s.push_str("Q\n");
            pdf::append_content(&mut doc, page, s.into_bytes(), &res)?;
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

/// The watermark picture as an image XObject (with its transparency kept as an SMask).
fn load_mark(doc: &mut Document, req: &Request) -> Result<(ObjectId, f64, f64), FileError> {
    let path = req.options.get("image").and_then(|v| v.as_str().map(String::from).or_else(|| v["path"].as_str().map(String::from)));
    let path = path.filter(|p| !p.is_empty()).ok_or_else(|| FileError::new("no_image"))?;
    let im = img::load(std::path::Path::new(&path), req)?.img;
    Ok(pdf::image_xobject(doc, &im))
}
