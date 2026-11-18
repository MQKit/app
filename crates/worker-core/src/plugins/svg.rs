//! SVG and vectorize, both ways by file type. Vector files become PNG, JPG, WebP or PDF:
//! SVG drawn with resvg, EMF and WMF with GDI+ (as Windows and Office draw them),
//! Illustrator files through the PDF they carry. Pictures are traced into SVG drawings
//! (visioncortex, the engine behind VTracer).

use super::image_convert::IMAGE_INPUTS;
use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{edge, gdiplus, img, winrt};
use contract::{codes, opt, t, FileError, FileRef, Request};
use image::{DynamicImage, RgbaImage};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;

pub struct Svg;

const VECTOR: &[&str] = &["svg", "svgz", "emf", "wmf", "ai"];
/// Largest picture side traced; bigger ones are scaled down first (the SVG keeps the size).
const TRACE_MAX: u32 = 2400;
/// Largest side drawn, in pixels.
const RENDER_MAX: f64 = 16384.0;

impl Plugin for Svg {
    fn id(&self) -> &'static str {
        "svg"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = VECTOR.iter().chain(IMAGE_INPUTS.iter()).copied().collect();
        let is_vector = json!({ "file": "kind", "eq": "vector", "quant": "any" });
        let is_picture = json!({ "file": "kind", "eq": "picture", "quant": "any" });
        json!({
            "name": t("SVG și vectorizare", "SVG and vectorize"),
            "summary": t("Vectori în ambele sensuri", "Vectors both ways"),
            "keywords": { "ro": ["svg", "emf", "wmf", "illustrator", "vectorizare", "svg în png", "imagine în svg"],
                          "en": ["svg", "emf", "wmf", "illustrator", "vectorize", "svg to png", "image to svg"] },
            "requires": [ { "engine": "libreoffice", "when": { "all": [ { "field": "format", "eq": "pdf" }, { "file": "format", "in": ["emf", "wmf"], "quant": "any" } ] } } ],
            "category": "image",
            "icon": "vector-bezier-2",
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["png", "jpg", "webp", "pdf", "svg"],
                        "nameVariables": [ { "id": "width", "label": t("lățime", "width") }, { "id": "height", "label": t("înălțime", "height") } ],
                        "replaceOriginalAllowed": false },
            "analysis": { "fast": ["kind", "format", "width", "height"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "heavy", "diskFactor": 4.0 },
            "presets": [],
            "groups": [
                { "id": "draw", "label": t("Fișierele vectoriale", "Vector files"), "order": 0 },
                { "id": "trace", "label": t("Imaginile de vectorizat", "Pictures to trace"), "order": 1 }
            ],
            "fields": [
                { "key": "format", "type": "select", "group": "draw", "label": t("Fișierele vectoriale în", "Vector files to"), "default": "png", "display": "chips", "showIf": is_vector,
                  "options": [ opt("png", "PNG", "PNG"), opt("jpg", "JPG", "JPG"), opt("webp", "WebP", "WebP"), opt("pdf", "PDF", "PDF") ] },
                { "key": "size", "type": "select", "group": "draw", "label": t("Mărime", "Size"), "default": 2,
                  "showIf": { "all": [ is_vector, { "field": "format", "ne": "pdf" } ] },
                  "options": [ opt(1, "Ca în fișier (1×)", "As in the file (1×)"), opt(2, "Dublă (2×)", "Double (2×)"), opt(4, "De 4 ori (4×)", "4 times (4×)") ],
                  "allowCustom": { "type": "number", "min": 16, "max": 16384, "unit": "px" },
                  "help": t("Sau o lățime în pixeli; un desen vectorial rămâne clar la orice mărime.", "Or a width in pixels; a vector drawing stays sharp at any size.") },
                { "key": "transparent", "type": "toggle", "group": "draw", "default": true, "label": t("Fundal transparent", "Transparent background"),
                  "showIf": { "all": [ is_vector, { "field": "format", "in": ["png", "webp"] } ] } },
                { "key": "background", "type": "color", "group": "draw", "default": "#ffffff", "label": t("Culoarea fundalului", "Background color"),
                  "showIf": { "all": [ is_vector, { "any": [ { "field": "format", "eq": "jpg" },
                      { "all": [ { "field": "format", "in": ["png", "webp"] }, { "field": "transparent", "eq": false } ] } ] } ] } },
                { "key": "quality", "type": "range", "group": "draw", "label": t("Calitate", "Quality"), "min": 1, "max": 100, "step": 1, "unit": "%", "default": 92,
                  "showIf": { "all": [ is_vector, { "field": "format", "in": ["jpg", "webp"] } ] } },
                { "key": "colors", "type": "select", "group": "trace", "label": t("Culori", "Colors"), "default": "color", "display": "chips", "showIf": is_picture,
                  "options": [ opt("color", "Color", "Color"), opt("bw", "O singură culoare", "One color") ] },
                { "key": "ink", "type": "color", "group": "trace", "default": "#000000", "label": t("Culoarea desenului", "Drawing color"),
                  "showIf": { "all": [ is_picture, { "field": "colors", "eq": "bw" } ] } },
                { "key": "detail", "type": "select", "group": "trace", "label": t("Detalii", "Detail"), "default": "balanced", "showIf": is_picture,
                  "options": [ opt("simple", "Puține (forme mari)", "Few (big shapes)"), opt("balanced", "Echilibrat", "Balanced"), opt("fine", "Multe (fidel)", "Many (faithful)") ] },
                { "key": "curves", "type": "select", "group": "trace", "label": t("Contururi", "Outlines"), "default": "spline", "showIf": is_picture,
                  "options": [ opt("spline", "Curbe netede", "Smooth curves"), opt("polygon", "Linii drepte", "Straight lines"), opt("none", "Pixeli", "Pixels") ] }
            ],
            "rules": [
                { "id": "photo", "severity": "info", "when": is_picture,
                  "message": t("Vectorizarea iese cel mai bine la logo-uri, desene și pictograme; o fotografie dă un SVG mare.",
                               "Tracing works best on logos, drawings and icons; a photo gives a large SVG.") },
                { "id": "metafile_pdf", "severity": "info",
                  "when": { "all": [ { "field": "format", "eq": "pdf" }, { "file": "format", "in": ["emf", "wmf"], "quant": "any" } ] },
                  "message": t("EMF și WMF devin PDF cu LibreOffice, descărcat o singură dată; desenul rămâne vectorial.",
                               "EMF and WMF become PDF with LibreOffice, downloaded once; the drawing stays vector.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "ai_not_pdf": t("Fișierul Illustrator nu are PDF în el. Salvează-l din nou în Illustrator cu „Create PDF Compatible File” bifat.",
                                "This Illustrator file has no PDF inside. Save it again in Illustrator with “Create PDF Compatible File” ticked.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let path = f.input_path();
        if is_vector(path) {
            let mut p = Map::new();
            p.insert("kind".into(), json!("vector"));
            let ext = ext_of(path);
            p.insert("format".into(), json!(if ext == "svgz" { "svg" } else { ext.as_str() }));
            return match Vector::open(path) {
                Ok(v) => {
                    p.insert("width".into(), json!(v.size.0.round() as u32));
                    p.insert("height".into(), json!(v.size.1.round() as u32));
                    Analysis::ok(p)
                }
                Err(e) => Analysis { status: "failed", props: p, warnings: vec![], code: Some(e.code) },
            };
        }
        match img::analyze(path) {
            Ok(mut p) => {
                p.insert("kind".into(), json!("picture"));
                Analysis::ok(p)
            }
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
        let dir = ctx.file_dir(&f.id)?;
        let (out, ext, w, h) = if is_vector(path) { draw(ctx, f, &dir)? } else { trace(ctx, f, &dir)? };
        ctx.progress(&f.id, 1.0, "save", 2, 2);
        ctx.emit_output(&f.id, &out, ext, json!({ "width": w, "height": h }), in_bytes);
        Ok(Value::Null)
    }
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn is_vector(path: &Path) -> bool {
    VECTOR.contains(&ext_of(path).as_str())
}

/// A vector file, opened by the engine that draws its kind, with its size in
/// device-independent pixels (1/96 in).
struct Vector {
    kind: Kind,
    size: (f64, f64),
}

enum Kind {
    Svg(Box<resvg::usvg::Tree>),
    Metafile,
    /// An Illustrator file saved with PDF compatibility: its first artboard.
    Illustrator(winrt::Pdf),
}

impl Vector {
    fn open(path: &Path) -> Result<Vector, FileError> {
        match ext_of(path).as_str() {
            "emf" | "wmf" => Ok(Vector { kind: Kind::Metafile, size: gdiplus::size(path)? }),
            "ai" => {
                let mut head = [0u8; 5];
                let n = std::io::Read::read(&mut std::fs::File::open(path).map_err(super::io_err)?, &mut head).unwrap_or(0);
                if n < 5 || &head != b"%PDF-" {
                    return Err(FileError::new("ai_not_pdf"));
                }
                let doc = winrt::Pdf::open(path, None)?;
                let size = doc.page_size(0)?;
                Ok(Vector { kind: Kind::Illustrator(doc), size })
            }
            _ => {
                let tree = load_tree(path)?;
                let size = (tree.size().width() as f64, tree.size().height() as f64);
                Ok(Vector { kind: Kind::Svg(Box::new(tree)), size })
            }
        }
    }

    /// Drawn at `w` × `h` pixels, transparent where the drawing has nothing.
    fn raster(&self, path: &Path, w: u32, h: u32) -> Result<DynamicImage, FileError> {
        match &self.kind {
            Kind::Svg(tree) => {
                let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
                let (sx, sy) = (w as f32 / self.size.0 as f32, h as f32 / self.size.1 as f32);
                resvg::render(tree, resvg::tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
                let rgba = RgbaImage::from_raw(w, h, pixmap.take_demultiplied()).ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
                Ok(DynamicImage::ImageRgba8(rgba))
            }
            Kind::Metafile => Ok(DynamicImage::ImageRgba8(gdiplus::render(path, w, h)?)),
            Kind::Illustrator(doc) => {
                let png = doc.render_png(0, 96.0 * w as f64 / self.size.0, true)?;
                image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(img::img_err)
            }
        }
    }
}

/// A vector file (SVG, SVGZ, EMF, WMF, AI) drawn with its long side `long` pixels, and
/// its own size: for tools that need a picture of it (icons).
pub(super) fn render_vector(path: &Path, long: u32) -> Result<(DynamicImage, (f64, f64)), FileError> {
    let v = Vector::open(path)?;
    let (sw, sh) = v.size;
    let s = long as f64 / sw.max(sh).max(1e-6);
    let (w, h) = ((sw * s).round().max(1.0) as u32, (sh * s).round().max(1.0) as u32);
    Ok((v.raster(path, w, h)?, v.size))
}

/// Whether a file is one of the vector formats `render_vector` draws.
pub(super) fn is_vector_file(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "svg" | "svgz" | "emf" | "wmf" | "ai")
}

fn load_tree(path: &Path) -> Result<resvg::usvg::Tree, FileError> {
    use resvg::usvg::ImageHrefResolver;
    let data = std::fs::read(path).map_err(super::io_err)?;
    // Pictures a drawing links to are read only from its own folder (and below), like a
    // browser would: a drawing received from someone can't pull in a photo from elsewhere
    // on this PC. Nothing is ever fetched from the internet.
    let base = path.parent().and_then(|p| std::fs::canonicalize(p).ok());
    let files = ImageHrefResolver::default_string_resolver();
    let resolver = ImageHrefResolver {
        resolve_data: ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(move |href: &str, opts: &resvg::usvg::Options| {
            let base = base.as_ref()?;
            let local = href.strip_prefix("file:///").or_else(|| href.strip_prefix("file://")).unwrap_or(href);
            if local.contains("://") {
                return None;
            }
            let full = std::fs::canonicalize(base.join(local)).ok()?;
            if !full.starts_with(base) {
                return None;
            }
            files(full.to_str()?, opts)
        }),
    };
    let opt = resvg::usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        fontdb: crate::common::font::system(),
        image_href_resolver: resolver,
        ..Default::default()
    };
    resvg::usvg::Tree::from_data(&data, &opt).map_err(|e| FileError::with(codes::CORRUPT_FILE, json!({ "message": e.to_string() })))
}

/// A vector file drawn to a picture, or made into a PDF that keeps it as vectors.
fn draw(ctx: &Ctx, f: &FileRef, dir: &Path) -> Result<(std::path::PathBuf, &'static str, u32, u32), FileError> {
    let req = ctx.req;
    let path = f.input_path();
    let vector = Vector::open(path)?;
    ctx.check_cancel()?;
    let (sw, sh) = vector.size;
    let format = req.str_or("format", "png");
    // LibreOffice reports its own progress from the start.
    if !(format == "pdf" && matches!(vector.kind, Kind::Metafile)) {
        ctx.progress(&f.id, 0.3, "draw", 2, 2);
    }
    if format == "pdf" {
        let out = dir.join("result.pdf");
        match &vector.kind {
            // An Illustrator file with PDF compatibility already is a PDF.
            Kind::Illustrator(_) => {
                std::fs::copy(path, &out).map_err(super::io_err)?;
            }
            Kind::Metafile => {
                let produced = super::word_to_pdf_lo::convert_with_lo(ctx, f, "pdf:draw_pdf_Export", dir)?;
                fit_page_to_drawing(ctx, f, &produced, &out)?;
                let _ = std::fs::remove_file(&produced);
            }
            Kind::Svg(tree) => svg_pdf(ctx, f, tree, &out)?,
        }
        return Ok((out, "pdf", sw.round() as u32, sh.round() as u32));
    }
    let size = req.f64_or("size", 2.0);
    let mut scale = if size > 8.0 { size / sw } else { size.max(0.1) };
    scale = scale.min(RENDER_MAX / sw.max(sh));
    // The pixel budget too (a 16384-pixel square is 268 MP): WebP can't go past 16383.
    let max_pixels = req.limits.max_megapixels.max(1) as f64 * 1e6;
    scale = scale.min((max_pixels / (sw * sh).max(1e-6)).sqrt());
    if format == "webp" {
        scale = scale.min(16383.0 / sw.max(sh));
    }
    let (w, h) = ((sw * scale).round().max(1.0) as u32, (sh * scale).round().max(1.0) as u32);
    let mut im = vector.raster(path, w, h)?;
    let (w, h) = (im.width(), im.height());
    ctx.check_cancel()?;
    let bg = img::parse_color(req.str_or("background", "#ffffff"));
    if format != "jpg" && !req.bool_or("transparent", true) {
        im = DynamicImage::ImageRgb8(img::flatten(&im, bg));
    }
    let q = req.f64_or("quality", 92.0).clamp(1.0, 100.0) as u8;
    let (data, ext) = match format {
        "jpg" => (img::encode_jpeg(&im, q, None, None, bg)?, "jpg"),
        "webp" => (img::encode_webp(&im, q, false)?, "webp"),
        _ => (img::encode_png(&im, false, 100)?, "png"),
    };
    let out = dir.join(format!("result.{ext}"));
    write_file(&out, &data)?;
    Ok((out, ext, w, h))
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Where the first `/Artifact BMC … EMC` block starts, and its length up to `EMC`.
fn artifact_block(content: &[u8]) -> Option<(usize, usize)> {
    let a = find_bytes(content, b"/Artifact BMC")?;
    let len = find_bytes(&content[a..], b"EMC")?;
    Some((a, len))
}

fn replace_bytes(src: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

/// LibreOffice puts a metafile on a whole sheet of paper (Letter or A4, by locale),
/// shrunk to fit if large. This drops the white sheet it paints and sets the page to
/// where the drawing really is, found by drawing the page on a transparent background.
fn fit_page_to_drawing(ctx: &Ctx, _f: &FileRef, produced: &Path, out: &Path) -> Result<(), FileError> {
    use lopdf::Object;
    let mut doc = crate::common::pdf::open(produced, None)?;
    let page = *doc.get_pages().values().next().ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
    let content = doc.get_page_content(page);
    // The sheet: the first artifact block, a white rectangle the size of the page. Its
    // fills become "n" (end the path, paint nothing), so q/Q stay paired. Searched in the
    // bytes themselves: positions found in a lossy text copy are off after any non-UTF-8 byte.
    if let Some((a, len)) = artifact_block(&content) {
        let block = replace_bytes(&replace_bytes(&content[a..a + len], b"\nf*\n", b"\nn\n"), b"\nf\n", b"\nn\n");
        let mut cleaned = content[..a].to_vec();
        cleaned.extend_from_slice(&block);
        cleaned.extend_from_slice(&content[a + len..]);
        doc.change_page_content(page, cleaned).map_err(|_| FileError::new(codes::CORRUPT_FILE))?;
    }
    // Drawn from memory: no copy is left behind if drawing fails.
    let mut probe = Vec::new();
    crate::common::pdf::write(&mut doc, &mut probe, false)?;
    let png = winrt::Pdf::open_bytes(&probe)?.render_png(0, 144.0, true)?;
    drop(probe);
    ctx.check_cancel()?;
    let im = image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(img::img_err)?.to_rgba8();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in im.enumerate_pixels() {
        if p[3] > 8 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x0 <= x1 {
        let media = crate::common::pdf::media_box(&doc, page);
        let (left, top) = (media.0, media.3);
        // Pixels per point, from what was actually drawn.
        let (kx, ky) = (im.width() as f64 / (media.2 - media.0).max(1.0), im.height() as f64 / (media.3 - media.1).max(1.0));
        let r = |v: f64| Object::Real(((v * 100.0).round() / 100.0) as f32);
        let bbox = vec![r(left + x0 as f64 / kx - 0.5), r(top - (y1 + 1) as f64 / ky - 0.5), r(left + (x1 + 1) as f64 / kx + 0.5), r(top - y0 as f64 / ky + 0.5)];
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page) {
            d.set("MediaBox", bbox.clone());
            d.set("CropBox", bbox);
        }
    }
    crate::common::pdf::save(&mut doc, out, false)
}

/// An SVG printed to PDF by Edge at its own size, after usvg normalized it (text as
/// outlines, images inside), so it stays vector.
pub(super) fn svg_pdf(ctx: &Ctx, f: &FileRef, tree: &resvg::usvg::Tree, out: &Path) -> Result<(), FileError> {
    let (sw, sh) = (tree.size().width() as f64, tree.size().height() as f64);
    {
        // Normalized by usvg (text as outlines, images inside), then printed at its own size.
        let svg = tree.to_string(&resvg::usvg::WriteOptions::default());
        let svg_path = ctx.work_dir.join(format!("{}.svg", f.id));
        write_file(&svg_path, svg.as_bytes())?;
        let html = format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><style>@page {{ size: {sw}px {sh}px; margin: 0; }} html, body {{ margin: 0; padding: 0; overflow: hidden; }} img {{ display: block; width: {sw}px; height: {sh}px; }}</style></head><body><img src="{}"></body></html>"#,
            edge::file_url(&svg_path)
        );
        let html_path = ctx.work_dir.join(format!("{}.html", f.id));
        write_file(&html_path, html.as_bytes())?;
        edge::print(ctx, &f.id, &html_path, out, Duration::from_secs(60))?;
    }
    Ok(())
}

/// A picture traced into an SVG of filled shapes.
fn trace(ctx: &Ctx, f: &FileRef, dir: &Path) -> Result<(std::path::PathBuf, &'static str, u32, u32), FileError> {
    use visioncortex::color_clusters::{KeyingAction, Runner, RunnerConfig, HIERARCHICAL_MAX};
    use visioncortex::{Color, ColorImage, PathSimplifyMode};
    let req = ctx.req;
    let loaded = img::load(f.input_path(), req)?;
    let (ow, oh) = (loaded.img.width(), loaded.img.height());
    let im = img::fit_long_side(loaded.img, TRACE_MAX).to_rgba8();
    let (w, h) = (im.width() as usize, im.height() as usize);
    ctx.check_cancel()?;
    ctx.progress(&f.id, 0.3, "trace", 2, 2);

    // (speckle size in px, colour precision bits, layer difference, corner angle)
    let (speckle, precision, layers, corner) = match req.str_or("detail", "balanced") {
        "simple" => (8usize, 5, 32, 60.0),
        "fine" => (2, 8, 8, 60.0),
        _ => (4, 6, 16, 60.0),
    };
    let mode = match req.str_or("curves", "spline") {
        "polygon" => PathSimplifyMode::Polygon,
        "none" => PathSimplifyMode::None,
        _ => PathSimplifyMode::Spline,
    };
    let (corner, length, iterations, splice) = (corner / 180.0 * std::f64::consts::PI, 4.0, 10, 45.0 / 180.0 * std::f64::consts::PI);
    let mut paths: Vec<(visioncortex::CompoundPath, String)> = Vec::new();

    if req.str_or("colors", "color") == "bw" {
        // Dark (and not transparent) pixels become the drawing.
        let mut bin = visioncortex::BinaryImage::new_w_h(w, h);
        for (x, y, p) in im.enumerate_pixels() {
            let lum = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
            bin.set_pixel(x as usize, y as usize, p[3] >= 128 && lum < 128.0);
        }
        let [r, g, b] = img::parse_color(req.str_or("ink", "#000000"));
        let ink = format!("#{r:02x}{g:02x}{b:02x}");
        let clusters = bin.to_clusters(false);
        for i in 0..clusters.len() {
            let cluster = clusters.get_cluster(i);
            if cluster.size() >= speckle * speckle {
                paths.push((cluster.to_compound_path(mode, corner, length, iterations, splice), ink.clone()));
            }
        }
    } else {
        let mut ci = ColorImage { pixels: im.as_raw().to_vec(), width: w, height: h };
        // Transparent areas get a colour not in the picture, then are left out.
        let key = if has_transparency(&im) { unused_color(&im).unwrap_or_default() } else { Color::default() };
        if key != Color::default() {
            for (x, y, p) in im.enumerate_pixels() {
                if p[3] == 0 {
                    ci.set_pixel(x as usize, y as usize, &key);
                }
            }
        }
        let runner = Runner::new(
            RunnerConfig {
                diagonal: layers == 0,
                hierarchical: HIERARCHICAL_MAX,
                batch_size: 25600,
                good_min_area: speckle * speckle,
                good_max_area: w * h,
                is_same_color_a: 8 - precision,
                is_same_color_b: 1,
                deepen_diff: layers,
                hollow_neighbours: 1,
                key_color: key,
                keying_action: KeyingAction::Discard,
            },
            ci,
        );
        let clusters = runner.run();
        let view = clusters.view();
        for &index in view.clusters_output.iter().rev() {
            let cluster = view.get_cluster(index);
            let path = cluster.to_compound_path(&view, false, mode, corner, length, iterations, splice);
            paths.push((path, cluster.residue_color().to_hex_string()));
        }
    }
    ctx.check_cancel()?;

    let mut svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{ow}\" height=\"{oh}\" viewBox=\"0 0 {w} {h}\">\n"
    );
    for (path, color) in &paths {
        let (d, offset) = path.to_svg_string(true, visioncortex::PointF64::default(), Some(2));
        if d.is_empty() {
            continue;
        }
        svg.push_str(&format!("<path d=\"{d}\" fill=\"{color}\" transform=\"translate({},{})\"/>\n", offset.x, offset.y));
    }
    svg.push_str("</svg>\n");
    let out = dir.join("result.svg");
    write_file(&out, svg.as_bytes())?;
    Ok((out, "svg", ow, oh))
}

fn has_transparency(im: &RgbaImage) -> bool {
    let transparent = im.pixels().filter(|p| p[3] == 0).count();
    transparent > 0 && transparent * 20 >= (im.width() as usize).min(im.height() as usize)
}

/// A colour that no pixel has, to mark the transparent areas.
fn unused_color(im: &RgbaImage) -> Option<visioncortex::Color> {
    let mut seen = vec![false; 1 << 24];
    for p in im.pixels() {
        seen[((p[0] as usize) << 16) | ((p[1] as usize) << 8) | p[2] as usize] = true;
    }
    // Skip black (0,0,0): visioncortex reads it as "no key".
    (1..seen.len()).find(|&i| !seen[i]).map(|i| visioncortex::Color::new((i >> 16) as u8, (i >> 8) as u8, i as u8))
}

#[cfg(test)]
mod artifact_tests {
    #[test]
    fn the_sheet_is_found_in_the_bytes() {
        // A non-UTF-8 byte before the block: a lossy text copy shifts positions by 2.
        let mut c = b"q (\xE9t\xE9) Tj Q\n".to_vec();
        let at = c.len();
        c.extend_from_slice(b"/Artifact BMC\n0 0 612 792 re\nf\nEMC\nq 1 0 0 1 0 0 cm\nf*\nQ\n");
        let (a, len) = super::artifact_block(&c).unwrap();
        assert_eq!(a, at);
        assert!(c[a + len..].starts_with(b"EMC"));
        let block = super::replace_bytes(&super::replace_bytes(&c[a..a + len], b"\nf*\n", b"\nn\n"), b"\nf\n", b"\nn\n");
        assert_eq!(block, b"/Artifact BMC\n0 0 612 792 re\nn\n".to_vec());
    }
}
