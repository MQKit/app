//! Tool 2, "exact look" entry: each PDF page becomes a full-page picture in the
//! Word document, scaled into Word's page limits. For presentations and designs
//! (huge pages, vector backgrounds, little text) that Word's own PDF reflow and
//! pdf2docx take apart. Also the analysis entry of `pdf-to-word`: page sizes and
//! whether the PDF looks like a design (`designLike`).

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{img, pdf};
use contract::{codes, pages, FileError, FileRef, Request};
use lopdf::{Document, Object};
use serde_json::{json, Value};
use std::io::Write;

pub struct PdfToWordExact;

/// Word's largest page side: 22 inches.
const WORD_MAX_PT: f64 = 22.0 * 72.0;
const EMU_PER_PT: f64 = 12700.0;
const TWIPS_PER_PT: f64 = 20.0;

/// Does the page have a coloured (not white) filled rectangle covering at least
/// half of it? That is what a slide or a design looks like; a letter with a
/// table has thin borders only.
fn has_background(doc: &Document, page: lopdf::ObjectId, area: f64) -> bool {
    let Ok(content) = doc.get_and_decode_page_content(page) else { return false };
    let num = |o: &Object| match o {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => *r as f64,
        _ => 0.0,
    };
    let mut pending: f64 = 0.0;
    let light_from = |ops: &[Object], kind: &str| -> bool {
        let v: Vec<f64> = ops.iter().map(num).collect();
        match (kind, v.len()) {
            ("g", 1) => v[0] > 0.92,
            ("rg", 3) => v.iter().all(|c| *c > 0.92),
            ("k", 4) => v.iter().all(|c| *c < 0.08),
            _ => false,
        }
    };
    let mut light = false;
    for op in &content.operations {
        match op.operator.as_str() {
            "g" | "rg" | "k" => light = light_from(&op.operands, op.operator.as_str()),
            "sc" | "scn" => {
                let v: Vec<f64> = op.operands.iter().map(num).collect();
                light = !v.is_empty() && v.iter().all(|c| *c > 0.92);
            }
            "re" if op.operands.len() == 4 => {
                pending += (num(&op.operands[2]) * num(&op.operands[3])).abs();
            }
            "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                if !light && pending >= area * 0.5 {
                    return true;
                }
                pending = 0.0;
            }
            "n" | "S" | "s" => pending = 0.0,
            _ => {}
        }
    }
    false
}

impl Plugin for PdfToWordExact {
    fn id(&self) -> &'static str {
        "pdf-to-word-exact"
    }
    fn hidden(&self) -> bool {
        true
    }
    fn manifest(&self) -> Value {
        json!({})
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, mut props, code) = pdf::analyze(f, req);
        if status == "ok" {
            if let Ok(doc) = pdf::open_file(f, req) {
                let pages = doc.get_pages();
                let sizes: Vec<(f64, f64)> = pages.values().filter_map(|p| pdf::page_box(&doc, *p)).collect();
                let max_w = sizes.iter().map(|s| s.0).fold(0.0, f64::max);
                let max_h = sizes.iter().map(|s| s.1).fold(0.0, f64::max);
                let oversize = sizes.iter().any(|(w, h)| w.max(*h) > WORD_MAX_PT + 1.0);
                // A sample of pages: a coloured background over half a page means a design.
                let sample: Vec<_> = pages.values().take(4).copied().collect();
                let backgrounds = sample
                    .iter()
                    .filter(|id| pdf::page_box(&doc, **id).map(|(w, h)| has_background(&doc, **id, w * h)).unwrap_or(false))
                    .count();
                let design = oversize || backgrounds * 2 >= sample.len().max(1);
                props.insert("format".into(), json!("pdf"));
                props.insert("maxPageWidthPt".into(), json!(max_w.round()));
                props.insert("maxPageHeightPt".into(), json!(max_h.round()));
                props.insert("oversizePages".into(), json!(oversize));
                props.insert("backgroundPages".into(), json!(backgrounds));
                props.insert("designLike".into(), json!(design));
            }
        }
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        let pdfdoc = crate::common::pdf::windows_reader(ctx, f)?;
        let total = pdfdoc.page_count();
        let wanted = pages::parse_set(ctx.req.str_or("pages", "all"), total)
            .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": bad, "pageCount": total })))?;
        // The picture's name in Word's selection pane, in the user's language.
        let page_word = if ctx.req.locale.as_deref().unwrap_or("").starts_with("ro") { "Pagina" } else { "Page" };
        let quality = ctx.req.str_or("imageQuality", "high");
        let dpi_target = if quality == "smaller" { 120.0 } else { 200.0 };

        let mut media: Vec<(String, Vec<u8>)> = Vec::new();
        let mut body = String::new();
        let n = wanted.len();
        for (i, page) in wanted.iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, i as f64 / n as f64 * 0.9, "export", 1, 2);
            // The page as shown (rotated, cropped), like the picture: in points.
            let (pw, ph) = pdfdoc.page_size(page - 1).map(|(w, h)| (w * 0.75, h * 0.75)).unwrap_or((595.0, 842.0));
            // Fit Word's limit, keep the shape.
            let scale = (WORD_MAX_PT / pw.max(ph)).min(1.0);
            let (w, h) = (pw * scale, ph * scale);
            // Render so the picture has about `dpi_target` on the Word page, at most 4000 px wide.
            let render_dpi = (dpi_target * scale).min(4000.0 / (pw / 72.0)).max(36.0);
            let png = pdfdoc.render_png(page - 1, render_dpi, false)?;
            let im = image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(img::img_err)?;
            let q = if quality == "smaller" { 78 } else { 90 };
            let jpg = img::encode_jpeg(&im, q, None, None, [255, 255, 255])?;
            let name = format!("page{}.jpg", i + 1);
            media.push((name, jpg));
            let rid = format!("rIdImg{}", i + 1);
            let (cx, cy) = ((w * EMU_PER_PT).round() as i64, (h * EMU_PER_PT).round() as i64);
            let id = i + 1;
            let anchor = format!(
                r#"<w:r><w:rPr><w:sz w:val="2"/></w:rPr><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="{id}" behindDoc="1" locked="1" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="{id}" name="{page_word} {page}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="page{id}.jpg"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"#
            );
            let sect = sect_pr(w, h);
            if i + 1 < n {
                body.push_str(&format!(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/>{sect}</w:pPr>{anchor}</w:p>"#));
            } else {
                body.push_str(&format!(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/></w:pPr>{anchor}</w:p>{sect}"#));
            }
        }
        ctx.progress(&f.id, 0.92, "save", 2, 2);
        let out = ctx.file_dir(&f.id)?.join("result.docx");
        write_docx(&out, &body, &media).map_err(super::io_err)?;
        ctx.progress(&f.id, 1.0, "save", 2, 2);
        ctx.emit_output(&f.id, &out, "docx", json!({ "pages": n }), in_bytes);
        Ok(json!({ "engine": "exact" }))
    }
}

fn sect_pr(w: f64, h: f64) -> String {
    let (tw, th) = ((w * TWIPS_PER_PT).round() as i64, (h * TWIPS_PER_PT).round() as i64);
    let orient = if w > h { r#" w:orient="landscape""# } else { "" };
    format!(r#"<w:sectPr><w:pgSz w:w="{tw}" w:h="{th}"{orient}/><w:pgMar w:top="0" w:right="0" w:bottom="0" w:left="0" w:header="0" w:footer="0" w:gutter="0"/></w:sectPr>"#)
}

fn write_docx(path: &std::path::Path, body: &str, media: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(path)?;
    let mut z = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let err = |e: zip::result::ZipError| std::io::Error::other(e.to_string());
    z.start_file("[Content_Types].xml", opts).map_err(err)?;
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="jpg" ContentType="image/jpeg"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#)?;
    z.start_file("_rels/.rels", opts).map_err(err)?;
    z.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#)?;
    let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (i, (name, _)) in media.iter().enumerate() {
        rels.push_str(&format!(r#"<Relationship Id="rIdImg{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{name}"/>"#, i + 1));
    }
    rels.push_str("</Relationships>");
    z.start_file("word/_rels/document.xml.rels", opts).map_err(err)?;
    z.write_all(rels.as_bytes())?;
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>"#
    );
    z.start_file("word/document.xml", opts).map_err(err)?;
    z.write_all(doc.as_bytes())?;
    for (name, data) in media {
        z.start_file(format!("word/media/{name}"), stored).map_err(err)?;
        z.write_all(data)?;
    }
    z.finish().map_err(err)?;
    Ok(())
}
