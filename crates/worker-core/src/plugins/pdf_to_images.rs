//! Tool 6: PDF to images, rendered by Windows (Windows.Data.Pdf), page by page.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{img, pdf};
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use serde_json::{json, Value};

pub struct PdfToImages;

impl Plugin for PdfToImages {
    fn id(&self) -> &'static str {
        "pdf-to-images"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("PDF în imagini", "PDF to images"),
            "summary": t("Fiecare pagină o poză", "Each page an image"),
            "keywords": { "ro": ["pdf în jpg", "pdf în png", "pagini", "poze"], "en": ["pdf to jpg", "pdf to png", "pages", "pictures"] },
            "category": "pdf",
            "icon": "photo",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "multiPerInput", "extensions": ["jpg", "png", "webp"],
                        "nameVariables": [ { "id": "page", "label": t("pagina", "page") }, { "id": "total", "label": t("total pagini", "page total") } ],
                        "replaceOriginalAllowed": false },
            "analysis": { "fast": ["needsPassword", "pageCount"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 3.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "jpg", "display": "chips",
                  "options": [ opt("jpg", "JPG", "JPG"), opt("png", "PNG", "PNG"), opt("webp", "WebP", "WebP") ] },
                { "key": "dpi", "type": "select", "label": t("Rezoluție", "Resolution"), "default": 150,
                  "options": [ opt(72, "Ecran (72 dpi)", "Screen (72 dpi)"), opt(150, "Bună (150 dpi)", "Good (150 dpi)"),
                               opt(300, "Tipar (300 dpi)", "Print (300 dpi)") ],
                  "allowCustom": { "type": "number", "min": 36, "max": 600, "unit": "dpi" } },
                { "key": "pages", "type": "pages", "syntax": "set", "label": t("Pagini", "Pages"), "default": "all",
                  "placeholder": t("toate", "all"), "help": t("De exemplu 1-3, 5, z (ultima)", "For example 1-3, 5, z (last)") },
                { "key": "quality", "type": "range", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 90,
                  "showIf": { "field": "format", "in": ["jpg", "webp"] } },
                { "key": "transparent", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Fundal transparent", "Transparent background"), "showIf": { "field": "format", "eq": "png" } }
            ],
            "rules": [
                { "id": "many_pages", "when": { "file": "pageCount", "gt": 200 }, "severity": "info",
                  "message": t("Un document mare dă multe imagini; poți alege doar unele pagini.", "A large document gives many images; you can pick only some pages.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}-{page}", "{name}-{page}"), "outputDir": "subfolder", "subfolderName": "{name}" } },
            "errors": {
                "dpi_reduced": t("Unele pagini sunt foarte mari: au ieșit la {dpi} dpi, cât încape într-o imagine.",
                                 "Some pages are very large: they came out at {dpi} dpi, as much as fits in one picture.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        let doc = crate::common::pdf::windows_reader(ctx, f)?;
        let total = doc.page_count();
        let wanted = pages::parse_set(ctx.req.str_or("pages", "all"), total)
            .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": bad, "pageCount": total })))?;
        let format = ctx.req.str_or("format", "jpg");
        let dpi = ctx.req.f64_or("dpi", 150.0).clamp(36.0, 600.0);
        let q = ctx.req.f64_or("quality", 90.0) as u8;
        let transparent = format == "png" && ctx.req.bool_or("transparent", false);
        let dir = ctx.file_dir(&f.id)?;
        let n = wanted.len();
        // A picture can't be any size: the pixel budget, and WebP's 16383-pixel side.
        // JPG and WebP are encoded from a decoded copy of the page: a smaller budget.
        let budget = if format == "png" { 250.0 } else { 120.0 };
        let max_pixels = (ctx.req.limits.max_megapixels.max(1) as f64).min(budget) * 1e6;
        let max_side = if format == "webp" { 16383.0 } else { 65000.0 };
        let mut lowest: Option<f64> = None;
        for (i, page) in wanted.into_iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, i as f64 / n as f64, "export", 1, 1);
            let (wi, hi) = doc.page_size(page - 1).map(|(w, h)| (w / 96.0, h / 96.0)).unwrap_or((8.5, 11.0));
            let fit = (max_pixels / (wi * hi).max(1e-6)).sqrt().min(max_side / wi.max(hi).max(1e-6)).floor();
            let page_dpi = dpi.min(fit).max(1.0);
            if page_dpi < dpi {
                lowest = Some(lowest.map_or(page_dpi, |l: f64| l.min(page_dpi)));
            }
            let png = doc.render_png(page - 1, page_dpi, transparent)?;
            ctx.progress(&f.id, (i as f64 + 0.6) / n as f64, "export", 1, 1);
            let (data, ext) = match format {
                "png" => (png, "png"),
                other => {
                    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&png), image::ImageFormat::Png);
                    reader.limits(img::limits(ctx.req.limits.max_megapixels.max(250)));
                    let im = reader.decode().map_err(img::img_err)?;
                    if other == "webp" {
                        (img::encode_webp(&im, q, false)?, "webp")
                    } else {
                        (img::encode_jpeg(&im, q, None, None, [255, 255, 255])?, "jpg")
                    }
                }
            };
            let out = dir.join(format!("page-{page}.{ext}"));
            write_file(&out, &data)?;
            ctx.emit_output(&f.id, &out, ext, json!({ "page": page, "total": total, "n": i + 1 }), if i == 0 { in_bytes } else { 0 });
        }
        if let Some(l) = lowest {
            ctx.out.warning(Some(&f.id), "dpi_reduced", Some(json!({ "dpi": l.round() })));
        }
        ctx.progress(&f.id, 1.0, "export", 1, 1);
        Ok(Value::Null)
    }
}
