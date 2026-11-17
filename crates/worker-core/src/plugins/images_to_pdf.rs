//! Tool 4: images to PDF. JPGs go in without recompression; EXIF orientation is respected.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::img::{self, Kind};
use crate::common::pdf;
use contract::{codes, opt, t, FileError, FileRef, Request};
use lopdf::{dictionary, Document, Object, Stream};
use serde_json::{json, Value};
use std::io::Write;

pub struct ImagesToPdf;

const MM: f64 = 72.0 / 25.4;

impl Plugin for ImagesToPdf {
    fn id(&self) -> &'static str {
        "images-to-pdf"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Imagini în PDF", "Images to PDF"),
            "summary": t("Poze și scanări într-un PDF", "Photos and scans into a PDF"),
            "keywords": { "ro": ["jpg în pdf", "poze", "scanări", "fotografii", "png în pdf"], "en": ["jpg to pdf", "photos", "scans", "png to pdf"] },
            "category": "pdf",
            "icon": "file-type-pdf",
            "requires": [ { "engine": "ffmpeg", "when": { "file": "heifCodec", "eq": "missing", "quant": "any" } } ],
            "input": { "extensions": super::image_convert::IMAGE_INPUTS, "multiple": true, "folders": true, "mode": "combined", "ordered": true },
            "output": { "kind": "single", "extensions": ["pdf"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["format", "width", "height"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "thumbnails" },
            "resources": { "class": "light", "diskFactor": 1.5 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "pageSize", "type": "select", "label": t("Mărimea paginii", "Page size"), "default": "a4",
                  "options": [ opt("a4", "A4", "A4"), opt("letter", "Letter", "Letter"), opt("fit", "Cât imaginea", "Same as the image") ] },
                { "key": "orientation", "type": "select", "label": t("Orientare", "Orientation"), "default": "auto",
                  "showIf": { "field": "pageSize", "ne": "fit" },
                  "options": [ opt("auto", "După imagine", "Follow the image"), opt("portrait", "Portret", "Portrait"), opt("landscape", "Peisaj", "Landscape") ] },
                { "key": "margin", "type": "select", "label": t("Margini", "Margins"), "default": 0,
                  "options": [ opt(0, "Fără", "None"), opt(10, "Mici (1 cm)", "Small (1 cm)"), opt(20, "Mari (2 cm)", "Large (2 cm)") ] },
                { "key": "onePdfPerImage", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Câte un PDF pentru fiecare imagine", "One PDF for each image") }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {}
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        match img::analyze(f.input_path()) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        if ctx.req.bool_or("onePdfPerImage", false) {
            for f in &ctx.req.files {
                if ctx.cancel.stop_after_current() {
                    break;
                }
                ctx.run_one(&f.id, || build(ctx, std::slice::from_ref(f), &f.id));
            }
        } else {
            let files = &ctx.req.files;
            ctx.run_one("*", || build(ctx, files, "*"));
        }
        Ok(())
    }
}

/// One picture ready to embed: the XObject stream, its stored pixel size, the EXIF
/// orientation the page must apply (1 = none) and its ICC profile, if it has one.
pub(crate) struct Picture {
    stream: Stream,
    w: u32,
    h: u32,
    pub(crate) orientation: u8,
    icc: Option<(Vec<u8>, u8)>,
}

impl Picture {
    /// The size as shown, after the orientation.
    pub(crate) fn shown(&self) -> (u32, u32) {
        if self.orientation >= 5 { (self.h, self.w) } else { (self.w, self.h) }
    }
}

fn image_stream(w: u32, h: u32, cs: &str, bits: i64, filter: &str, data: Vec<u8>) -> Stream {
    let mut s = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                      "ColorSpace" => cs, "BitsPerComponent" => bits, "Filter" => filter },
        data,
    );
    s.allows_compression = false;
    s
}

/// An ICC profile fit for the image: RGB for 3 components, gray for 1.
fn usable_icc(icc: Option<Vec<u8>>, comps: u8) -> Option<(Vec<u8>, u8)> {
    let icc = icc?;
    let space = icc.get(16..20)?;
    let ok = (comps == 3 && space == b"RGB ") || (comps == 1 && space == b"GRAY");
    (ok && icc.len() > 128 && icc.len() < 4_000_000).then_some((icc, comps))
}

/// The pictures of one file: one, or one per page of a multi-page TIFF.
pub(crate) fn embed(ctx: &Ctx, f: &FileRef) -> Result<Vec<Picture>, FileError> {
    let path = f.input_path();
    let kind = img::sniff(path);
    // JPEGs go in as they are, whatever their orientation (the page turns them): no quality loss.
    if kind == Kind::Jpeg {
        let data = std::fs::read(path).map_err(super::io_err)?;
        if let Some((w, h, comps)) = jpeg_info(&data) {
            if comps == 1 || comps == 3 {
                let icc = usable_icc(jpeg_icc(&data), comps);
                let cs = if comps == 1 { "DeviceGray" } else { "DeviceRGB" };
                let stream = image_stream(w, h, cs, 8, "DCTDecode", data);
                return Ok(vec![Picture { stream, w, h, orientation: exif_orientation(path), icc }]);
            }
        }
    }
    // TIFF: every page, through the Windows decoder (fax G3/G4, JPEG, palette, CMYK, 16 bits…),
    // colours managed to sRGB.
    if kind == Kind::Tiff {
        if let Ok((_, _, frames)) = crate::common::winrt::image_info(path) {
            let mut out = Vec::new();
            for i in 0..frames.max(1) {
                ctx.check_cancel()?;
                let Ok((w, h, rgba)) = crate::common::winrt::decode_image(path, i) else { break };
                let Some(im) = image::RgbaImage::from_raw(w, h, rgba) else { break };
                out.push(lossless(&image::DynamicImage::ImageRgba8(im), None)?);
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
    }
    let loaded = img::load(path, ctx.req)?;
    let photo = matches!(loaded.kind, Kind::Jpeg | Kind::Heic | Kind::Avif | Kind::Webp);
    if photo {
        let (w, h) = (loaded.img.width(), loaded.img.height());
        let jpg = img::encode_jpeg(&loaded.img, 92, None, None, [255, 255, 255])?;
        let gray = !loaded.img.color().has_color();
        let icc = usable_icc(loaded.icc.clone(), if gray { 1 } else { 3 });
        let stream = image_stream(w, h, if gray { "DeviceGray" } else { "DeviceRGB" }, 8, "DCTDecode", jpg);
        return Ok(vec![Picture { stream, w, h, orientation: 1, icc }]);
    }
    Ok(vec![lossless(&loaded.img, loaded.icc.clone())?])
}

/// Screenshots, drawings, scans: without loss (Flate), transparency flattened on white
/// (the page is white). Gray stays gray and black-and-white takes 1 bit per pixel.
fn lossless(im: &image::DynamicImage, icc: Option<Vec<u8>>) -> Result<Picture, FileError> {
    let rgb = img::flatten(im, [255, 255, 255]);
    let (w, h) = (rgb.width(), rgb.height());
    let raw = rgb.as_raw();
    let gray = raw.chunks_exact(3).all(|p| p[0] == p[1] && p[1] == p[2]);
    let zip = |bytes: &[u8]| -> Result<Vec<u8>, FileError> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(bytes).map_err(super::io_err)?;
        z.finish().map_err(super::io_err)
    };
    if gray {
        let g: Vec<u8> = raw.chunks_exact(3).map(|p| p[0]).collect();
        if g.iter().all(|v| *v == 0 || *v == 255) {
            // Rows packed 8 pixels to a byte, most significant bit first; 1 = white.
            let row = (w as usize).div_ceil(8);
            let mut bits = vec![0u8; row * h as usize];
            for (i, v) in g.iter().enumerate() {
                if *v == 255 {
                    let (y, x) = (i / w as usize, i % w as usize);
                    bits[y * row + x / 8] |= 0x80 >> (x % 8);
                }
            }
            let stream = image_stream(w, h, "DeviceGray", 1, "FlateDecode", zip(&bits)?);
            return Ok(Picture { stream, w, h, orientation: 1, icc: None });
        }
        let stream = image_stream(w, h, "DeviceGray", 8, "FlateDecode", zip(&g)?);
        return Ok(Picture { stream, w, h, orientation: 1, icc: usable_icc(icc, 1) });
    }
    let stream = image_stream(w, h, "DeviceRGB", 8, "FlateDecode", zip(raw)?);
    Ok(Picture { stream, w, h, orientation: 1, icc: usable_icc(icc, 3) })
}

/// Adds the picture's image object (with its colour profile, one object per profile).
pub(crate) fn add_picture(doc: &mut Document, pic: Picture, profiles: &mut std::collections::HashMap<Vec<u8>, lopdf::ObjectId>) -> lopdf::ObjectId {
    let mut stream = pic.stream;
    if let Some((icc, n)) = pic.icc {
        // One profile object for all the pictures that share it (photos from one camera).
        let id = *profiles.entry(icc.clone()).or_insert_with(|| {
            let mut s = Stream::new(dictionary! { "N" => n as i64, "Alternate" => if n == 1 { "DeviceGray" } else { "DeviceRGB" } }, icc);
            let _ = s.compress();
            doc.add_object(s)
        });
        stream.dict.set("ColorSpace", vec![Object::Name(b"ICCBased".to_vec()), Object::Reference(id)]);
    }
    doc.add_object(stream)
}

/// The `cm` matrix that draws the image (a unit square) into the box at (x, y), dw × dh
/// as shown, applying the EXIF orientation.
pub(crate) fn placement(orientation: u8, x: f64, y: f64, dw: f64, dh: f64) -> [f64; 6] {
    match orientation {
        2 => [-dw, 0.0, 0.0, dh, x + dw, y],
        3 => [-dw, 0.0, 0.0, -dh, x + dw, y + dh],
        4 => [dw, 0.0, 0.0, -dh, x, y + dh],
        5 => [0.0, -dh, -dw, 0.0, x + dw, y + dh],
        6 => [0.0, -dh, dw, 0.0, x, y + dh],
        7 => [0.0, dh, dw, 0.0, x, y],
        8 => [0.0, dh, -dw, 0.0, x + dw, y],
        _ => [dw, 0.0, 0.0, dh, x, y],
    }
}

fn build(ctx: &Ctx, files: &[FileRef], unit: &str) -> Result<Value, FileError> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids: Vec<Object> = Vec::new();
    let mut in_bytes = 0u64;
    let size = ctx.req.str_or("pageSize", "a4");
    let orient = ctx.req.str_or("orientation", "auto");
    let margin = ctx.req.f64_or("margin", 0.0) * MM;
    let mut profiles: std::collections::HashMap<Vec<u8>, lopdf::ObjectId> = std::collections::HashMap::new();

    for (i, f) in files.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.progress(unit, i as f64 / files.len() as f64 * 0.9, "encode", 1, 2);
        in_bytes += file_size(f.input_path());
        let pictures = embed(ctx, f).map_err(|mut e| {
            e.details = Some(json!({ "file": f.id }));
            e
        })?;
        for pic in pictures {
        let (w, h) = pic.shown();
        let orientation = pic.orientation;
        let img_id = add_picture(&mut doc, pic, &mut profiles);
        // Page size in points.
        let (pw, ph) = match size {
            "fit" => {
                // 150 dpi keeps phone photos at a sensible page size.
                let s = 72.0 / 150.0;
                (w as f64 * s + 2.0 * margin, h as f64 * s + 2.0 * margin)
            }
            other => {
                let (a, b) = if other == "letter" { (612.0, 792.0) } else { (595.28, 841.89) };
                let landscape = match orient {
                    "landscape" => true,
                    "portrait" => false,
                    _ => w > h,
                };
                if landscape {
                    (b, a)
                } else {
                    (a, b)
                }
            }
        };
        let (aw, ah) = ((pw - 2.0 * margin).max(1.0), (ph - 2.0 * margin).max(1.0));
        let scale = (aw / w as f64).min(ah / h as f64);
        let (dw, dh) = (w as f64 * scale, h as f64 * scale);
        let (x, y) = ((pw - dw) / 2.0, (ph - dh) / 2.0);
        let [a, b, c, d, e, g] = placement(orientation, x, y, dw, dh);
        let content = format!("q {a:.3} {b:.3} {c:.3} {d:.3} {e:.3} {g:.3} cm /Im0 Do Q");
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw as f32), Object::Real(ph as f32)],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img_id } },
        });
        kids.push(page_id.into());
        }
    }
    let count = kids.len();
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count as i64 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    ctx.progress(unit, 0.92, "restructure", 2, 2);
    doc.compress();
    let dir = ctx.file_dir(if unit == "*" { "combined" } else { unit })?;
    let path = dir.join("result.pdf");
    pdf::save(&mut doc, &path, false)?;
    ctx.progress(unit, 1.0, "restructure", 2, 2);
    ctx.emit_output(unit, &path, "pdf", json!({ "pages": count }), in_bytes);
    Ok(Value::Null)
}

/// (width, height, components) from the JPEG SOF marker.
fn jpeg_info(d: &[u8]) -> Option<(u32, u32, u8)> {
    let mut p = 2;
    while p + 4 < d.len() {
        if d[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = d[p + 1];
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) || m == 0xFF {
            p += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            // PDF readers show baseline and progressive Huffman JPEGs at 8 bits; lossless,
            // arithmetic-coded or 12-bit ones are decoded and embedded another way.
            if !matches!(m, 0xC0..=0xC2) || *d.get(p + 4)? != 8 {
                return None;
            }
            let s = p + 4;
            let h = u16::from_be_bytes([*d.get(s + 1)?, *d.get(s + 2)?]) as u32;
            let w = u16::from_be_bytes([*d.get(s + 3)?, *d.get(s + 4)?]) as u32;
            let c = *d.get(s + 5)?;
            return (w > 0 && h > 0).then_some((w, h, c));
        }
        p += 2 + len;
    }
    None
}

/// The ICC profile of a JPEG: its APP2 "ICC_PROFILE" chunks, in order.
fn jpeg_icc(d: &[u8]) -> Option<Vec<u8>> {
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    let mut p = 2;
    while p + 4 < d.len() {
        if d[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = d[p + 1];
        if m == 0xDA || m == 0xD9 {
            break; // image data: no more metadata
        }
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) || m == 0xFF {
            p += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        let seg = d.get(p + 4..p + 2 + len)?;
        if m == 0xE2 && seg.len() > 14 && &seg[..12] == b"ICC_PROFILE\0" {
            chunks.push((seg[12], &seg[14..]));
        }
        p += 2 + len;
    }
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|c| c.0);
    Some(chunks.into_iter().flat_map(|c| c.1.iter().copied()).collect())
}

fn exif_orientation(path: &std::path::Path) -> u8 {
    use image::ImageDecoder;
    image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.into_decoder().ok())
        .and_then(|mut d| d.orientation().ok())
        .map(|o| o.to_exif())
        .unwrap_or(1)
}
