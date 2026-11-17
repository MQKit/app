//! PDF to PowerPoint: every page becomes a slide holding the page as a picture, rendered
//! by Windows. The slides keep the page's shape, or are 16:9 or 4:3 with the page fitted
//! or filling the slide.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{img, pdf};
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use serde_json::{json, Value};
use std::io::Write;

pub struct PdfToPptx;

/// EMU (the Office length unit) per device-independent pixel (1/96 in).
const EMU_PER_DIP: f64 = 9525.0;
const EMU_PER_INCH: f64 = 914400.0;
/// PowerPoint accepts slides from 1 to 56 inches on a side.
const MIN_SIDE: f64 = EMU_PER_INCH;
const MAX_SIDE: f64 = 56.0 * EMU_PER_INCH;

impl Plugin for PdfToPptx {
    fn id(&self) -> &'static str {
        "pdf-to-pptx"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("PDF în PowerPoint", "PDF to PowerPoint"),
            "summary": t("Fiecare pagină un slide", "Each page a slide"),
            "keywords": { "ro": ["pdf în pptx", "prezentare", "slide", "powerpoint"], "en": ["pdf to pptx", "presentation", "slides", "powerpoint"] },
            "category": "pdf",
            "icon": "file-type-ppt",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pptx"],
                        "nameVariables": [ { "id": "slides", "label": t("număr de slide-uri", "slide count") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["needsPassword", "pageCount"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 2.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "slideSize", "type": "select", "label": t("Mărimea slide-urilor", "Slide size"), "default": "page", "display": "chips",
                  "options": [ opt("page", "Ca pagina", "Like the page"), opt("16:9", "16:9", "16:9"), opt("4:3", "4:3", "4:3") ] },
                { "key": "fit", "type": "select", "label": t("Pagina pe slide", "Page on the slide"), "default": "fit",
                  "showIf": { "field": "slideSize", "ne": "page" },
                  "options": [ opt("fit", "Întreagă, cu margini", "Whole, with borders"), opt("fill", "Umple slide-ul (taie marginile)", "Fill the slide (crops the edges)") ] },
                { "key": "background", "type": "color", "label": t("Culoarea marginilor", "Border color"), "default": "#ffffff",
                  "showIf": { "all": [ { "field": "slideSize", "ne": "page" }, { "field": "fit", "eq": "fit" } ] } },
                { "key": "pages", "type": "pages", "syntax": "set", "label": t("Pagini", "Pages"), "default": "all",
                  "placeholder": t("toate", "all"), "help": t("De exemplu 1-3, 5, z (ultima)", "For example 1-3, 5, z (last)") },
                { "key": "sharpness", "type": "select", "label": t("Claritate", "Sharpness"), "default": 2880,
                  "options": [ opt(1920, "Ecran (Full HD)", "Screen (Full HD)"), opt(2880, "Clară", "Sharp"), opt(3840, "Foarte clară (4K)", "Very sharp (4K)") ] },
                { "key": "imageFormat", "type": "select", "advanced": true, "group": "more", "label": t("Imaginile paginilor", "Page images"), "default": "auto",
                  "options": [ opt("auto", "Automat (cel mai mic)", "Automatic (smallest)"), opt("jpg", "JPG", "JPG"), opt("png", "PNG (fără pierderi)", "PNG (lossless)") ] }
            ],
            "rules": [
                { "id": "pictures", "severity": "info", "when": { "field": "slideSize", "in": ["page", "16:9", "4:3"] },
                  "message": t("Paginile devin imagini: arată exact ca în PDF, dar textul nu se poate edita.",
                               "Pages become pictures: they look exactly as in the PDF, but the text can't be edited.") },
                { "id": "many_pages", "when": { "file": "pageCount", "gt": 200 }, "severity": "info",
                  "message": t("O prezentare cu multe slide-uri iese mare; poți alege doar unele pagini.", "A deck with many slides comes out large; you can pick only some pages.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {}
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "export", 1, 2);
        let doc = crate::common::pdf::windows_reader(ctx, f)?;
        let total = doc.page_count();
        let wanted = pages::parse_set(req.str_or("pages", "all"), total)
            .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": bad, "pageCount": total })))?;
        let first = doc.page_size(wanted.first().copied().unwrap_or(1).saturating_sub(1))?;
        let size_mode = req.str_or("slideSize", "page");
        let (sw, sh) = slide_size(size_mode, first);
        let fill = size_mode != "page" && req.str_or("fit", "fit") == "fill";
        let background = (size_mode != "page" && !fill).then(|| img::parse_color(req.str_or("background", "#ffffff")));
        let format = req.str_or("imageFormat", "auto");
        // Pixels across the slide width; the same density for every page.
        let px = req.f64_or("sharpness", 2880.0).clamp(960.0, 3840.0);
        let dpi_per_slide_inch = px / (sw / EMU_PER_INCH);

        let n = wanted.len();
        let mut slides = Vec::with_capacity(n);
        for (i, page) in wanted.into_iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, 0.9 * i as f64 / n as f64, "export", 1, 2);
            let (pw, ph) = doc.page_size(page - 1)?;
            let (pw, ph) = (pw * EMU_PER_DIP, ph * EMU_PER_DIP);
            // The page's scale on the slide: whole inside, or covering it.
            let scale = if fill { (sw / pw).max(sh / ph) } else { (sw / pw).min(sh / ph) };
            let (iw, ih) = (pw * scale, ph * scale);
            let dpi = (dpi_per_slide_inch * iw / pw).clamp(24.0, 600.0);
            let png = doc.render_png(page - 1, dpi, false)?;
            ctx.check_cancel()?;
            let (data, ext) = encode(png, format)?;
            // Cropping (fill) as fractions of the picture, in 1/1000 of a percent.
            let crop = if fill {
                let cx = (((iw - sw) / 2.0) / iw * 100000.0).round().max(0.0) as i64;
                let cy = (((ih - sh) / 2.0) / ih * 100000.0).round().max(0.0) as i64;
                Some((cx, cy))
            } else {
                None
            };
            let (x, y, w, h) = if fill { (0.0, 0.0, sw, sh) } else { ((sw - iw) / 2.0, (sh - ih) / 2.0, iw, ih) };
            slides.push(Slide { page, data, ext, rect: [x as i64, y as i64, w as i64, h as i64], crop });
        }

        ctx.progress(&f.id, 0.9, "save", 2, 2);
        let out = ctx.file_dir(&f.id)?.join("result.pptx");
        let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        write_pptx(&out, &title, (sw as i64, sh as i64), background, &slides).map_err(super::io_err)?;
        ctx.progress(&f.id, 1.0, "save", 2, 2);
        ctx.emit_output(&f.id, &out, "pptx", json!({ "slides": n }), in_bytes);
        Ok(Value::Null)
    }
}

struct Slide {
    page: u32,
    data: Vec<u8>,
    ext: &'static str,
    /// x, y, width, height in EMU.
    rect: [i64; 4],
    /// Left/right and top/bottom crop, in 1/1000 of a percent.
    crop: Option<(i64, i64)>,
}

/// Slide size in EMU: the first page's shape (kept within PowerPoint's limits) or a
/// standard screen shape.
fn slide_size(mode: &str, first: (f64, f64)) -> (f64, f64) {
    match mode {
        "16:9" => (12192000.0, 6858000.0),
        "4:3" => (9144000.0, 6858000.0),
        _ => {
            let (w, h) = (first.0.max(1.0) * EMU_PER_DIP, first.1.max(1.0) * EMU_PER_DIP);
            let down = (MAX_SIDE / w).min(MAX_SIDE / h).min(1.0);
            let up = (MIN_SIDE / w).max(MIN_SIDE / h).max(1.0);
            let k = if down < 1.0 { down } else { up };
            ((w * k).round(), (h * k).round())
        }
    }
}

/// The rendered page as JPG or PNG; "auto" keeps whichever is smaller.
fn encode(png: Vec<u8>, format: &str) -> Result<(Vec<u8>, &'static str), FileError> {
    if format == "png" {
        return Ok((png, "png"));
    }
    let im = image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(img::img_err)?;
    // A page of text and line art (a few colours and their smoothed edges): a small
    // palette keeps the letters crisp at a third of a JPEG's size.
    if format == "auto" {
        if let Some(small) = palette_png(&im.to_rgb8()) {
            return Ok((small, "png"));
        }
    }
    let jpg = img::encode_jpeg(&im, 90, None, None, [255, 255, 255])?;
    if format == "auto" && png.len() <= jpg.len() {
        return Ok((png, "png"));
    }
    Ok((jpg, "jpg"))
}

/// The page with at most 32 colours, when that many cover nearly all of it (98%): every
/// other pixel (an edge's in-between shade) takes the nearest one. None for photos.
fn palette_png(rgb: &image::RgbImage) -> Option<Vec<u8>> {
    use std::collections::HashMap;
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    for p in rgb.pixels() {
        *counts.entry(p.0).or_insert(0) += 1;
        if counts.len() > 50_000 {
            return None; // a photo
        }
    }
    let mut by_use: Vec<([u8; 3], u32)> = counts.into_iter().collect();
    by_use.sort_unstable_by_key(|u| std::cmp::Reverse(u.1));
    let keep = by_use.len().min(32);
    let covered: u64 = by_use[..keep].iter().map(|c| c.1 as u64).sum();
    let total = rgb.width() as u64 * rgb.height() as u64;
    if covered * 100 < total * 98 {
        return None;
    }
    let palette: Vec<[u8; 3]> = by_use[..keep].iter().map(|c| c.0).collect();
    let mut index: HashMap<[u8; 3], u8> = palette.iter().enumerate().map(|(i, c)| (*c, i as u8)).collect();
    let nearest = |c: [u8; 3]| -> u8 {
        let d = |p: &[u8; 3]| (0..3).map(|k| (p[k] as i32 - c[k] as i32).pow(2)).sum::<i32>();
        (0..palette.len()).min_by_key(|i| d(&palette[*i])).unwrap_or(0) as u8
    };
    let bits: u8 = if palette.len() <= 2 { 1 } else if palette.len() <= 4 { 2 } else if palette.len() <= 16 { 4 } else { 8 };
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let per_byte = 8 / bits as usize;
    let row_len = w.div_ceil(per_byte);
    let mut data = vec![0u8; row_len * h];
    for (y, row) in rgb.rows().enumerate() {
        for (x, p) in row.enumerate() {
            let i = *index.entry(p.0).or_insert_with(|| nearest(p.0));
            let shift = 8 - bits as usize * (x % per_byte + 1);
            data[y * row_len + x / per_byte] |= i << shift;
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(match bits {
            1 => png::BitDepth::One,
            2 => png::BitDepth::Two,
            4 => png::BitDepth::Four,
            _ => png::BitDepth::Eight,
        });
        enc.set_palette(palette.iter().flatten().copied().collect::<Vec<u8>>());
        enc.set_compression(png::Compression::High);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(&data).ok()?;
    }
    Some(out)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument";
const EMPTY_TREE: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

fn rels(items: &[(String, &str, String)]) -> String {
    let mut s = format!(r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (id, kind, target) in items {
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{kind}" Target="{target}"/>"#));
    }
    s.push_str("</Relationships>");
    s
}

fn theme() -> String {
    let clr = |name: &str, hex: &str| format!(r#"<a:{name}><a:srgbClr val="{hex}"/></a:{name}>"#);
    let fill = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#.repeat(3);
    let line = r#"<a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#.repeat(3);
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>".repeat(3);
    let font = |face: &str| format!(r#"<a:latin typeface="{face}"/><a:ea typeface=""/><a:cs typeface=""/>"#);
    format!(
        r#"{XML}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>{}{}{}{}{}{}{}{}{}{}</a:clrScheme><a:fontScheme name="Office"><a:majorFont>{}</a:majorFont><a:minorFont>{}</a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst>{fill}</a:fillStyleLst><a:lnStyleLst>{line}</a:lnStyleLst><a:effectStyleLst>{effect}</a:effectStyleLst><a:bgFillStyleLst>{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#,
        clr("dk2", "44546A"), clr("lt2", "E7E6E6"), clr("accent1", "4472C4"), clr("accent2", "ED7D31"), clr("accent3", "A5A5A5"),
        clr("accent4", "FFC000"), clr("accent5", "5B9BD5"), clr("accent6", "70AD47"), clr("hlink", "0563C1"), clr("folHlink", "954F72"),
        font("Calibri Light"), font("Calibri"),
    )
}

fn slide_xml(s: &Slide, background: Option<[u8; 3]>) -> String {
    let bg = background
        .map(|[r, g, b]| format!(r#"<p:bg><p:bgPr><a:solidFill><a:srgbClr val="{r:02X}{g:02X}{b:02X}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>"#))
        .unwrap_or_default();
    let src = s.crop.map(|(cx, cy)| format!(r#"<a:srcRect l="{cx}" t="{cy}" r="{cx}" b="{cy}"/>"#)).unwrap_or_default();
    let [x, y, w, h] = s.rect;
    let page = s.page;
    format!(
        r#"{XML}<p:sld {NS}><p:cSld>{bg}<p:spTree>{EMPTY_TREE}<p:pic><p:nvPicPr><p:cNvPr id="2" name="Page {page}" descr="Page {page}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/>{src}<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn write_pptx(path: &std::path::Path, title: &str, (sw, sh): (i64, i64), background: Option<[u8; 3]>, slides: &[Slide]) -> std::io::Result<()> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(path)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let err = |e: zip::result::ZipError| std::io::Error::other(e.to_string());
    let put = |z: &mut zip::ZipWriter<_>, name: &str, data: &[u8], o: SimpleFileOptions| -> std::io::Result<()> {
        z.start_file(name, o).map_err(err)?;
        z.write_all(data)
    };

    let pml = format!("{CT}.presentationml");
    let mut types = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="jpg" ContentType="image/jpeg"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="{pml}.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="{pml}.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="{pml}.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="{CT}.theme+xml"/><Override PartName="/ppt/presProps.xml" ContentType="{pml}.presProps+xml"/><Override PartName="/ppt/viewProps.xml" ContentType="{pml}.viewProps+xml"/><Override PartName="/ppt/tableStyles.xml" ContentType="{pml}.tableStyles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="{CT}.extended-properties+xml"/>"#
    );
    for i in 1..=slides.len() {
        types.push_str(&format!(r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="{pml}.slide+xml"/>"#));
    }
    types.push_str("</Types>");
    put(&mut z, "[Content_Types].xml", types.as_bytes(), opts)?;

    put(&mut z, "_rels/.rels", rels(&[
        ("rId1".into(), &format!("{REL}/officeDocument"), "ppt/presentation.xml".into()),
        ("rId2".into(), "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties", "docProps/core.xml".into()),
        ("rId3".into(), &format!("{REL}/extended-properties"), "docProps/app.xml".into()),
    ]).as_bytes(), opts)?;
    put(&mut z, "docProps/core.xml", format!(
        r#"{XML}<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title></cp:coreProperties>"#, esc(title)
    ).as_bytes(), opts)?;
    put(&mut z, "docProps/app.xml", format!(
        r#"{XML}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Slides>{}</Slides></Properties>"#, slides.len()
    ).as_bytes(), opts)?;

    let mut pres_rels = vec![
        ("rId1".to_string(), format!("{REL}/slideMaster"), "slideMasters/slideMaster1.xml".to_string()),
        ("rId2".into(), format!("{REL}/theme"), "theme/theme1.xml".into()),
        ("rId3".into(), format!("{REL}/presProps"), "presProps.xml".into()),
        ("rId4".into(), format!("{REL}/viewProps"), "viewProps.xml".into()),
        ("rId5".into(), format!("{REL}/tableStyles"), "tableStyles.xml".into()),
    ];
    let mut ids = String::new();
    for i in 1..=slides.len() {
        pres_rels.push((format!("rId{}", i + 9), format!("{REL}/slide"), format!("slides/slide{i}.xml")));
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 255 + i, i + 9));
    }
    let pres_rels: Vec<(String, &str, String)> = pres_rels.iter().map(|(a, b, c)| (a.clone(), b.as_str(), c.clone())).collect();
    put(&mut z, "ppt/_rels/presentation.xml.rels", rels(&pres_rels).as_bytes(), opts)?;
    put(&mut z, "ppt/presentation.xml", format!(
        r#"{XML}<p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="{sw}" cy="{sh}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    ).as_bytes(), opts)?;
    put(&mut z, "ppt/presProps.xml", format!(r#"{XML}<p:presentationPr {NS}/>"#).as_bytes(), opts)?;
    put(&mut z, "ppt/viewProps.xml", format!(r#"{XML}<p:viewPr {NS}/>"#).as_bytes(), opts)?;
    put(&mut z, "ppt/tableStyles.xml", format!(
        r#"{XML}<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#
    ).as_bytes(), opts)?;
    put(&mut z, "ppt/theme/theme1.xml", theme().as_bytes(), opts)?;

    put(&mut z, "ppt/slideMasters/slideMaster1.xml", format!(
        r#"{XML}<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle/><p:bodyStyle/><p:otherStyle/></p:txStyles></p:sldMaster>"#
    ).as_bytes(), opts)?;
    put(&mut z, "ppt/slideMasters/_rels/slideMaster1.xml.rels", rels(&[
        ("rId1".into(), &format!("{REL}/slideLayout"), "../slideLayouts/slideLayout1.xml".into()),
        ("rId2".into(), &format!("{REL}/theme"), "../theme/theme1.xml".into()),
    ]).as_bytes(), opts)?;
    put(&mut z, "ppt/slideLayouts/slideLayout1.xml", format!(
        r#"{XML}<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
    ).as_bytes(), opts)?;
    put(&mut z, "ppt/slideLayouts/_rels/slideLayout1.xml.rels", rels(&[
        ("rId1".into(), &format!("{REL}/slideMaster"), "../slideMasters/slideMaster1.xml".into()),
    ]).as_bytes(), opts)?;

    for (i, s) in slides.iter().enumerate() {
        let n = i + 1;
        let media = format!("image{n}.{}", s.ext);
        put(&mut z, &format!("ppt/slides/slide{n}.xml"), slide_xml(s, background).as_bytes(), opts)?;
        put(&mut z, &format!("ppt/slides/_rels/slide{n}.xml.rels"), rels(&[
            ("rId1".into(), &format!("{REL}/slideLayout"), "../slideLayouts/slideLayout1.xml".into()),
            ("rId2".into(), &format!("{REL}/image"), format!("../media/{media}")),
        ]).as_bytes(), opts)?;
        put(&mut z, &format!("ppt/media/{media}"), &s.data, stored)?;
    }
    z.finish().map_err(err)?.flush()
}
