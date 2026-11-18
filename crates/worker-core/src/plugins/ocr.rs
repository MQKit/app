//! OCR with the engine built into Windows: scanned PDFs and photos become searchable
//! PDFs (an invisible text layer over the unchanged page) or text files.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{pdf, winrt};
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use serde_json::{json, Map, Value};

pub struct Ocr;

const IMAGES: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "heic", "heif"];
const DPI: f64 = 300.0;

impl Plugin for Ocr {
    fn id(&self) -> &'static str {
        "ocr"
    }

    fn manifest(&self) -> Value {
        let inputs: Vec<&str> = ["pdf"].iter().chain(IMAGES).copied().collect();
        json!({
            "name": t("OCR", "OCR"),
            "summary": t("Text din scanări", "Text from scans"),
            "keywords": { "ro": ["ocr", "scanare", "pdf căutabil", "recunoaștere text", "text din poză"], "en": ["ocr", "scan", "searchable pdf", "text recognition", "text from image"] },
            "category": "pdf",
            "icon": "text-scan-2",
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf", "txt"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["pageCount", "needsPassword"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 1.5 },
            "presets": [
                { "id": "searchable", "label": t("PDF în care se poate căuta", "Searchable PDF"), "default": true, "values": { "output": "pdf" } },
                { "id": "text", "label": t("Doar textul", "Just the text"), "values": { "output": "txt" } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "output", "type": "select", "label": t("Rezultat", "Result"), "default": "pdf", "display": "chips",
                  "options": [ opt("pdf", "PDF cu text (arată la fel)", "PDF with text (looks the same)"), opt("txt", "Fișier text", "Text file") ] },
                { "key": "language", "type": "select", "label": t("Limba textului", "Language of the text"), "default": "",
                  "optionsFrom": { "env": "windows.ocrLanguages" },
                  "help": t("Limbile vin din Windows: adaugă o limbă cu „Recunoaștere optică a caracterelor”.", "Languages come from Windows: add a language with “Optical character recognition”."),
                  "helpLink": { "label": t("Adaugă limbi în Setări", "Add languages in Settings"), "url": "ms-settings:regionlanguage" } },
                { "key": "pages", "type": "pages", "syntax": "set", "label": t("Pagini", "Pages"), "default": "all",
                  "placeholder": t("toate", "all"), "showIf": { "file": "pageCount", "gt": 1, "quant": "any" } },
                { "key": "redo", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Și paginile care au deja text", "Pages that already have text too") }
            ],
            "rules": [
                { "id": "no_own_language", "severity": "info", "when": { "env": "windows.ocrOwnLanguage", "eq": false },
                  "message": t("Windows nu poate recunoaște încă limba ta; literele specifice ei pot ieși greșit. O poți adăuga din Setări › Oră și limbă › Limbă.",
                               "Windows can't recognize your language yet; its own letters may come out wrong. You can add it in Settings › Time & language › Language.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_ocr", "{name}_ocr") } },
            "errors": {
                "no_ocr_language": t("Windows nu are nicio limbă instalată pentru recunoașterea textului.", "Windows has no language installed for text recognition.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let mut p = Map::new();
        let ext = ext_of(f);
        if ext == "pdf" {
            let (status, props, code) = pdf::analyze(f, req);
            for k in ["pageCount", "needsPassword"] {
                if let Some(v) = props.get(k) {
                    p.insert(k.into(), v.clone());
                }
            }
            if status != "ok" {
                return Analysis { status, props: p, warnings: vec![], code };
            }
        } else {
            // A multi-page TIFF (a scanned document) has a page per frame.
            let frames = winrt::image_info(f.input_path()).map(|i| i.2.max(1)).unwrap_or(1);
            p.insert("pageCount".into(), json!(frames));
        }
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        if winrt::ocr_languages().map(|l| l.is_empty()).unwrap_or(true) {
            return Err(FileError::new("no_ocr_language"));
        }
        let lang = req.str_or("language", "").to_string();
        let lang = (!lang.is_empty()).then_some(lang);
        let want_pdf = req.str_or("output", "pdf") == "pdf";
        let max_dim = winrt::ocr_max_dimension() as f64;

        // The pages to read, and what renders them for the OCR engine.
        let (mut doc, page_ids, renderer) = if ext_of(f) == "pdf" {
            let doc = pdf::open_file(f, req)?;
            let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
            (doc, ids, crate::common::pdf::windows_reader(ctx, f)?)
        } else {
            image_document(ctx, f)?
        };
        let total = page_ids.len() as u32;
        let chosen = pages::parse_set(req.str_or("pages", "all"), total)
            .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": bad, "pageCount": total })))?;
        let font = if want_pdf { Some(add_font(&mut doc)) } else { None };
        let mut text_out = String::new();
        let mut read = 0;
        let n = chosen.len().max(1);
        for (i, p) in chosen.iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, 0.05 + 0.85 * i as f64 / n as f64, "recognize", 2, 2);
            let page_id = page_ids[(*p - 1) as usize];
            if !req.bool_or("redo", false) && has_text(&doc, page_id) && want_pdf {
                continue;
            }
            let (w_pt, h_pt) = pdf::displayed_size(&doc, page_id);
            // Pixels for the OCR engine: 300 dpi, within the engine's limit.
            let dpi = DPI.min(max_dim * 72.0 / w_pt.max(h_pt));
            let png = renderer.render_png(p - 1, dpi, false)?;
            let (lines, turn) = read_any_way_up(&png, lang.as_deref())?;
            let (px_w, px_h) = png_size(&png);
            let k = w_pt / px_w.max(1) as f64; // points per pixel
            if want_pdf {
                let place = Placement { k, turn, w: px_w as f64, h: px_h as f64 };
                add_text_layer(&mut doc, page_id, &lines, &place, font.unwrap())?;
            }
            if total > 1 && !want_pdf {
                text_out.push_str(&format!("\r\n—— {} ——\r\n\r\n", crate::common::words::of(req.locale.as_deref()).page(p)));
            }
            for line in &lines {
                text_out.push_str(&line.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
                text_out.push_str("\r\n");
            }
            read += 1;
        }
        ctx.progress(&f.id, 0.92, "write", 2, 2);
        let dir = ctx.file_dir(&f.id)?;
        let out = if want_pdf {
            let out = dir.join("result.pdf");
            pdf::save(&mut doc, &out, false)?;
            out
        } else {
            let out = dir.join("result.txt");
            let mut data = vec![0xEF, 0xBB, 0xBF];
            data.extend_from_slice(text_out.as_bytes());
            write_file(&out, &data)?;
            out
        };
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        ctx.emit_output(&f.id, &out, if want_pdf { "pdf" } else { "txt" }, json!({}), in_bytes);
        Ok(json!({ "pagesRead": read }))
    }
}

fn ext_of(f: &FileRef) -> String {
    f.input_path().extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn png_size(png: &[u8]) -> (u32, u32) {
    if png.len() > 24 && png.starts_with(b"\x89PNG") {
        (u32::from_be_bytes([png[16], png[17], png[18], png[19]]), u32::from_be_bytes([png[20], png[21], png[22], png[23]]))
    } else {
        image::load_from_memory(png).map(|i| (i.width(), i.height())).unwrap_or((1, 1))
    }
}

/// A picture becomes a PDF, a page per picture (every page of a TIFF), the picture in as it
/// is (a JPEG unchanged, black and white in 1 bit), the page the size of the scanned paper
/// (the scan's own resolution, else 300 dpi). Its pages are rendered for the OCR engine
/// like a PDF's, so they are read the way they are shown.
fn image_document(ctx: &Ctx, f: &FileRef) -> Result<(Document, Vec<ObjectId>, winrt::Pdf), FileError> {
    let pictures = super::images_to_pdf::embed(ctx, f)?;
    let dpi = winrt::image_dpi(f.input_path()).filter(|d| (100.0..=1200.0).contains(d)).unwrap_or(DPI);
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut profiles = std::collections::HashMap::new();
    let mut ids = Vec::new();
    for pic in pictures {
        let (w, h) = pic.shown();
        let (wp, hp) = (w as f64 * 72.0 / dpi, h as f64 * 72.0 / dpi);
        let [a, b, c, d, e, g] = super::images_to_pdf::placement(pic.orientation, 0.0, 0.0, wp, hp);
        let img_id = super::images_to_pdf::add_picture(&mut doc, pic, &mut profiles);
        let content = doc.add_object(Stream::new(dictionary! {}, format!("q {a:.3} {b:.3} {c:.3} {d:.3} {e:.3} {g:.3} cm /Im0 Do Q").into_bytes()));
        ids.push(doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), Object::Real(wp as f32), Object::Real(hp as f32)],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img_id } }, "Contents" => content,
        }));
    }
    let kids: Vec<Object> = ids.iter().map(|id| Object::Reference(*id)).collect();
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
    let scan = ctx.work_dir.join(format!("ocr-{}.pdf", f.id));
    pdf::save(&mut doc, &scan, false)?;
    let renderer = winrt::Pdf::open(&scan, None)?;
    Ok((doc, ids, renderer))
}

/// How much real text a reading found: (letters in words, letters in clean words). Text
/// read the wrong way up comes out as scraps mixing letters, digits and signs ("110K",
/// "')xo)"), so few of its words are clean.
fn letters(lines: &[Vec<winrt::OcrWord>]) -> (usize, usize) {
    let (mut all, mut clean) = (0, 0);
    for w in lines.iter().flatten() {
        let n = w.text.chars().filter(|c| c.is_alphanumeric()).count();
        if n < 2 {
            continue;
        }
        all += n;
        let core = w.text.trim_matches(|c: char| "([{\"'\u{201E}\u{201C}\u{2018}\u{00AB}.,;:!?)]}\u{201D}\u{2019}\u{00BB}".contains(c));
        let digits = w.text.chars().any(|c| c.is_ascii_digit());
        if !core.is_empty() && !digits && core.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'' || c == '\u{2019}') {
            clean += n;
        }
    }
    (all, clean)
}

/// Enough clean text that the page was surely read the right way up.
fn reads_well((all, clean): (usize, usize)) -> bool {
    clean >= 200 && clean * 100 >= all * 85
}

/// Reads the page; when little real text comes out, also turned a quarter, half and three
/// quarters, for pages scanned sideways or upside down. The turn (clockwise degrees) that
/// read the most clean text wins.
fn read_any_way_up(png: &[u8], lang: Option<&str>) -> Result<(Vec<Vec<winrt::OcrWord>>, u32), FileError> {
    let upright = winrt::ocr_image(png, lang)?;
    let found = letters(&upright);
    if reads_well(found) {
        return Ok((upright, 0));
    }
    let Ok(im) = image::load_from_memory(png) else { return Ok((upright, 0)) };
    // Much text but little of it clean: most likely upside down.
    let order = if found.0 >= 200 { [180u32, 90, 270] } else { [90, 270, 180] };
    let mut best = (upright, 0u32, found.1);
    for turn in order {
        let turned = match turn {
            90 => im.rotate90(),
            180 => im.rotate180(),
            _ => im.rotate270(),
        };
        let mut data = std::io::Cursor::new(Vec::new());
        if turned.write_to(&mut data, image::ImageFormat::Bmp).is_err() {
            continue;
        }
        let lines = winrt::ocr_image(data.get_ref(), lang)?;
        let n = letters(&lines);
        // Clearly more text than the other ways: not just a few scraps more.
        if n.1 > best.2 * 3 / 2 + 10 {
            let done = reads_well(n);
            best = (lines, turn, n.1);
            if done {
                break;
            }
        }
    }
    Ok((best.0, best.1))
}

/// Where the words read go on the page.
struct Placement {
    /// Points per pixel.
    k: f64,
    /// Clockwise turn of the image that was read.
    turn: u32,
    /// The page image, unturned (pixels).
    w: f64,
    h: f64,
}

impl Placement {
    /// A point of the turned image (pixels, y down) on the page as shown (points, y up).
    fn to_page(&self, x: f64, y: f64, dh: f64) -> (f64, f64) {
        let (ox, oy) = match self.turn {
            90 => (y, self.h - x),
            180 => (self.w - x, self.h - y),
            270 => (self.w - y, x),
            _ => (x, y),
        };
        (ox * self.k, dh - oy * self.k)
    }
}

/// Does the page already have its text? What counts is how much text it draws, its form
/// XObjects included: a few words over a picture (a scanner's "Scanned with…" stamp, a page
/// number) don't, and the picture is still read.
fn has_text(doc: &Document, page: ObjectId) -> bool {
    let mut seen = TextSeen::default();
    let resources = doc.get_page_resources(page).ok().map(|(own, inherited)| {
        own.cloned().or_else(|| inherited.first().and_then(|id| doc.get_dictionary(*id).ok()).cloned()).unwrap_or_default()
    });
    seen.scan(doc, &doc.get_page_content(page), resources.as_ref(), 0);
    seen.chars >= 40 || (seen.chars > 0 && !seen.pictures)
}

#[derive(Default)]
struct TextSeen {
    chars: usize,
    pictures: bool,
}

impl TextSeen {
    fn scan(&mut self, doc: &Document, content: &[u8], resources: Option<&lopdf::Dictionary>, depth: u32) {
        let Ok(ops) = lopdf::content::Content::decode(content) else {
            // Can't be read: the old way, any text operator.
            if content.windows(2).any(|w| w == b"Tj" || w == b"TJ") {
                self.chars += 40;
            }
            return;
        };
        let xobjects = resources
            .and_then(|r| r.get(b"XObject").ok())
            .and_then(|x| doc.dereference(x).ok())
            .and_then(|(_, x)| x.as_dict().ok());
        for op in &ops.operations {
            match op.operator.as_str() {
                "Tj" | "'" | "\"" => self.chars += op.operands.last().and_then(|o| o.as_str().ok()).map_or(0, <[u8]>::len),
                "TJ" => {
                    if let Some(Ok(a)) = op.operands.first().map(Object::as_array) {
                        self.chars += a.iter().filter_map(|o| o.as_str().ok()).map(<[u8]>::len).sum::<usize>();
                    }
                }
                "BI" | "EI" => self.pictures = true,
                "Do" if depth < 8 => {
                    let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else { continue };
                    let Some(stream) = xobjects.and_then(|x| x.get(name).ok()).and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_stream().ok())
                    else {
                        continue;
                    };
                    match stream.dict.get(b"Subtype").and_then(Object::as_name).ok() {
                        Some(b"Image") => self.pictures = true,
                        Some(b"Form") => {
                            let own = stream.dict.get(b"Resources").ok().and_then(|r| doc.dereference(r).ok()).and_then(|(_, r)| r.as_dict().ok());
                            let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
                            self.scan(doc, &data, own.or(resources), depth + 1);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if self.chars >= 40 && self.pictures {
                return;
            }
        }
    }
}

/// A Type0 font with Unicode code points as CIDs and no glyphs: the text is invisible,
/// but it can be searched and copied (the ToUnicode map says what each code means).
fn add_font(doc: &mut Document) -> ObjectId {
    let mut cmap = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    let ranges: Vec<String> = (0u32..256).map(|hi| format!("<{hi:02X}00> <{hi:02X}FF> <{hi:02X}00>")).collect();
    for block in ranges.chunks(100) {
        cmap.push_str(&format!("{} beginbfrange\n{}\nendbfrange\n", block.len(), block.join("\n")));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let to_unicode = doc.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor", "FontName" => "GlyphLessFont", "Flags" => 5,
        "FontBBox" => vec![0.into(), 0.into(), 500.into(), 1000.into()], "ItalicAngle" => 0, "Ascent" => 1000, "Descent" => 0,
        "CapHeight" => 1000, "StemV" => 80,
    });
    let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "GlyphLessFont",
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => descriptor, "DW" => 500, "CIDToGIDMap" => "Identity",
    });
    doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "GlyphLessFont", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![cid.into()], "ToUnicode" => to_unicode,
    })
}

/// Invisible words over the page, each stretched to the width the engine measured.
fn add_text_layer(doc: &mut Document, page: ObjectId, lines: &[Vec<winrt::OcrWord>], place: &Placement, font: ObjectId) -> Result<(), FileError> {
    let k = place.k;
    if lines.iter().all(|l| l.is_empty()) {
        return Ok(());
    }
    let (_, dh) = pdf::displayed_size(doc, page);
    // Drawn in the coordinates of the page as shown, whatever its /Rotate.
    let mut s = format!("q {} BT 3 Tr
", pdf::display_cm(doc, page));
    for line in lines {
        for word in line {
            let units: Vec<u16> = word.text.encode_utf16().collect();
            if units.is_empty() {
                continue;
            }
            let size = (word.h * k).max(1.0);
            let natural = 0.5 * size * units.len() as f64;
            let scale = (word.w * k / natural * 100.0).clamp(1.0, 1000.0);
            // The baseline's start, and which way the line runs and up is, on the page.
            let base_y = word.y + word.h - 0.15 * word.h;
            let (x, y) = place.to_page(word.x, base_y, dh);
            let (ax, ay) = place.to_page(word.x + 1.0, base_y, dh);
            let (ux, uy) = place.to_page(word.x, base_y - 1.0, dh);
            let (a, b, c, d) = ((ax - x) / k, (ay - y) / k, (ux - x) / k, (uy - y) / k);
            let hex: String = units.iter().map(|u| format!("{u:04X}")).collect();
            s.push_str(&format!("/FOCR {size:.2} Tf {scale:.1} Tz {a:.0} {b:.0} {c:.0} {d:.0} {x:.2} {y:.2} Tm <{hex}> Tj\n"));
        }
    }
    s.push_str("ET Q\n");
    pdf::append_content(doc, page, s.into_bytes(), &[("Font", "FOCR", font.into())])?;
    Ok(())
}
