//! Fonts from Windows for text we draw ourselves: the system font list (shared with the
//! SVG renderer) and text placed in PDFs in any language and script. The text is shaped
//! (joined letters, right-to-left, marks) and only the glyphs used are embedded.

use contract::{codes, FileError};
use lopdf::{dictionary, Document, Object, Stream};
use resvg::usvg::fontdb;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

/// The system fonts, loaded once per worker.
pub fn system() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db.set_sans_serif_family("Segoe UI");
        db.set_serif_family("Times New Roman");
        db.set_monospace_family("Consolas");
        Arc::new(db)
    })
    .clone()
}

/// Windows fonts tried first, in order: Latin, Greek and Cyrillic first, then the fonts
/// Windows ships for other scripts. Any other installed font is tried after these.
const PREFERRED: &[&str] = &[
    "Arial", "Segoe UI", "Microsoft YaHei", "Yu Gothic", "Malgun Gothic", "Microsoft JhengHei", "Nirmala UI", "Leelawadee UI",
    "Segoe UI Historic", "Ebrima", "Gadugi", "Myanmar Text", "Khmer UI", "Lao UI", "Javanese Text", "Mongolian Baiti", "Segoe UI Symbol",
];

/// A TrueType font being placed in a PDF: shaped runs of text, and the glyphs used.
pub struct PdfFont {
    font: harfrust::Font,
    data: Vec<u8>,
    /// Where this face's table directory starts (not 0 inside a font collection).
    sfnt: usize,
    upem: f64,
    name: String,
    /// Where `hmtx` starts, and how many glyphs have their own advance in it.
    hmtx: usize,
    long_metrics: u16,
    /// Glyph → the text it stands for (for copying and searching).
    used: BTreeMap<u16, String>,
}

/// Shaped text: the operators that show it, and its width at size 1.
pub struct Run {
    ops: String,
    pub width: f64,
}

impl Run {
    /// The text-showing operators (inside BT … ET, after Tf and Tm).
    pub fn op(&self) -> &str {
        &self.ops
    }
}

/// One shaped glyph: its cluster (the letters it belongs to), and how it is placed.
struct Placed {
    gid: u16,
    cluster: usize,
    text: String,
    /// TJ adjustments before and after the glyph, in 1/1000 of the size.
    before: f64,
    after: f64,
}

impl PdfFont {
    /// The first font that has every letter of `texts` (or, failing that, the most).
    pub fn for_texts(texts: &[&str]) -> Result<PdfFont, FileError> {
        let db = system();
        let wanted: BTreeSet<char> = texts.iter().flat_map(|t| t.chars()).filter(|c| !c.is_whitespace() && !c.is_control()).collect();
        let regular = |f: &&fontdb::FaceInfo| f.style == fontdb::Style::Normal && f.weight == fontdb::Weight::NORMAL;
        let mut order: Vec<fontdb::ID> = Vec::new();
        for family in PREFERRED {
            order.extend(db.faces().filter(regular).filter(|f| f.families.iter().any(|(n, _)| n == family)).map(|f| f.id));
        }
        let rest: Vec<fontdb::ID> = db.faces().filter(regular).map(|f| f.id).filter(|id| !order.contains(id)).collect();
        order.extend(rest);
        let mut best: Option<(usize, PdfFont)> = None;
        for id in order {
            let Some(Some(font)) = db.with_face_data(id, |data, index| PdfFont::load(data.to_vec(), index)) else { continue };
            let missing = wanted.iter().filter(|c| !font.has(**c)).count();
            if missing == 0 {
                return Ok(font);
            }
            if best.as_ref().is_none_or(|(m, _)| missing < *m) {
                best = Some((missing, font));
            }
        }
        best.map(|(_, f)| f).ok_or_else(|| FileError::with(codes::ENGINE_FAILED, json!({ "component": "fonts" })))
    }

    /// A TrueType face that may be embedded (outlines in `glyf`, embedding allowed).
    fn load(data: Vec<u8>, index: u32) -> Option<PdfFont> {
        let sfnt = if data.starts_with(b"ttcf") { be32(&data, 12 + 4 * index as usize)? as usize } else { 0 };
        let tables = directory(&data, sfnt)?;
        let get = |tag: &[u8; 4]| tables.get(tag).copied();
        get(b"glyf")?;
        get(b"loca")?;
        let (head, _) = get(b"head")?;
        let upem = be16(&data, head + 18)? as f64;
        if let Some((os2, _)) = get(b"OS/2") {
            // fsType: 0x0002 = no embedding allowed, 0x0200 = only bitmaps may be embedded.
            if be16(&data, os2 + 8)? & 0x0202 != 0 {
                return None;
            }
        }
        let (hhea, _) = get(b"hhea")?;
        let (hmtx, _) = get(b"hmtx")?;
        let long_metrics = be16(&data, hhea + 34)?.max(1);
        // A subset is named with six capitals and "+" before the font's own name.
        let mut h = crc(&data[sfnt..(sfnt + 256).min(data.len())]);
        let tag: String = (0..6).map(|_| { let c = (b'A' + (h % 26) as u8) as char; h /= 26; c }).collect();
        let name = format!("{tag}+Font");
        let font = harfrust::Font::new(data.clone(), index)?;
        Some(PdfFont { font, data, sfnt, upem, name, hmtx, long_metrics, used: BTreeMap::new() })
    }

    fn has(&self, c: char) -> bool {
        self.font.charmap().map_unicode(c).is_some_and(|g| g.to_u32() != 0)
    }

    /// Shapes `text` and remembers the glyphs it uses. A glyph that is exactly one cluster
    /// of letters maps back to them (ToUnicode); clusters drawn with several glyphs, or
    /// with a glyph already standing for other letters, carry their letters as ActualText,
    /// the way browsers write PDFs.
    pub fn shape(&mut self, text: &str) -> Run {
        let (placed, width) = self.layout(text);
        let mut ops = String::new();
        let mut open = false;
        let mut i = 0;
        while i < placed.len() {
            let mut j = i + 1;
            while j < placed.len() && placed[j].cluster == placed[i].cluster {
                j += 1;
            }
            let group = &placed[i..j];
            let plain = group.len() == 1 && {
                let entry = self.used.entry(group[0].gid).or_default();
                if entry.is_empty() {
                    *entry = group[0].text.clone();
                }
                *entry == group[0].text
            };
            let glyphs: String = group.iter().map(tj_item).collect();
            if plain {
                if !open {
                    ops.push('[');
                    open = true;
                }
                ops.push_str(&glyphs);
            } else {
                for g in group {
                    self.used.entry(g.gid).or_default();
                }
                if open {
                    ops.push_str("] TJ ");
                    open = false;
                }
                let hex: String = group[0].text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                ops.push_str(&format!("/Span <</ActualText <FEFF{hex}>>> BDC [{glyphs}] TJ EMC "));
            }
            i = j;
        }
        if open {
            ops.push_str("] TJ");
        }
        Run { ops, width }
    }

    /// Glyphs in visual order (right-to-left runs reversed, letters joined), and the width.
    fn layout(&self, text: &str) -> (Vec<Placed>, f64) {
        let bidi = unicode_bidi::BidiInfo::new(text, None);
        let mut width = 0.0;
        let mut placed = Vec::new();
        let shaper = harfrust::ShaperFont::new(&self.font);
        let k = 1000.0 / self.upem;
        for para in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(para, para.range.clone());
            for run in runs {
                let sub = &text[run.clone()];
                if sub.is_empty() {
                    continue;
                }
                let ltr = levels[run.start].is_ltr();
                let mut buf = harfrust::Buffer::new();
                buf.push_str(sub);
                buf.set_direction(if ltr { harfrust::Direction::LeftToRight } else { harfrust::Direction::RightToLeft });
                buf.guess_segment_properties();
                if harfrust::shape(&shaper, &mut buf, harfrust::ShapeOptions::new()).is_err() {
                    continue;
                }
                let (infos, pos) = (buf.glyph_infos(), buf.glyph_positions());
                for i in 0..infos.len() {
                    let gid = infos[i].glyph_id as u16;
                    let start = infos[i].cluster as usize;
                    // A cluster's letters end where the next cluster (in logical order) starts.
                    let end = infos.iter().map(|g| g.cluster as usize).filter(|c| *c > start).min().unwrap_or(sub.len());
                    let (adv, dx) = (pos[i].x_advance as f64, pos[i].x_offset as f64);
                    // A TJ number moves what follows left by n/1000 of the size.
                    placed.push(Placed {
                        gid,
                        cluster: run.start + start,
                        text: sub.get(start..end).unwrap_or("").to_string(),
                        before: -dx * k,
                        after: (self.advance(gid) - adv + dx) * k,
                    });
                    width += adv / self.upem;
                }
            }
        }
        (placed, width)
    }

    /// The glyph's own advance from `hmtx`, in font units.
    fn advance(&self, gid: u16) -> f64 {
        let i = gid.min(self.long_metrics.saturating_sub(1)) as usize;
        be16(&self.data, self.hmtx + i * 4).unwrap_or(0) as f64
    }

    /// Adds the font's parts (only the glyphs used) to `doc` and returns the Type0 font
    /// dictionary, to store under an id reserved before the text was laid out.
    pub fn embed(&self, doc: &mut Document) -> Result<lopdf::Dictionary, FileError> {
        let file = self.subset().ok_or_else(|| FileError::with(codes::ENGINE_FAILED, json!({ "component": "fonts" })))?;
        let len = file.len() as i64;
        let mut stream = Stream::new(dictionary! { "Length1" => len }, file);
        let _ = stream.compress();
        let file_id = doc.add_object(stream);
        let k = 1000.0 / self.upem;
        let mut widths: Vec<Object> = Vec::new();
        for gid in self.used.keys() {
            widths.push(Object::Integer(*gid as i64));
            widths.push(Object::Array(vec![Object::Integer((self.advance(*gid) * k).round() as i64)]));
        }
        let descriptor = doc.add_object(dictionary! {
            "Type" => "FontDescriptor", "FontName" => Object::Name(self.name.clone().into_bytes()), "Flags" => 4,
            "FontBBox" => vec![(-1000).into(), (-1000).into(), 3000.into(), 3000.into()], "ItalicAngle" => 0,
            "Ascent" => 1000, "Descent" => -250, "CapHeight" => 700, "StemV" => 80, "FontFile2" => file_id,
        });
        let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => Object::Name(self.name.clone().into_bytes()),
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "FontDescriptor" => descriptor, "DW" => 1000, "W" => widths, "CIDToGIDMap" => "Identity",
        });
        let mut cmap = String::from("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /Adobe-Identity-UCS def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
        let mapped: Vec<(&u16, &String)> = self.used.iter().filter(|(_, t)| !t.is_empty()).collect();
        for chunk in mapped.chunks(100) {
            cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, text) in chunk {
                let hex: String = text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                cmap.push_str(&format!("<{gid:04X}> <{hex}>\n"));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end");
        let to_unicode = doc.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
        Ok(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "BaseFont" => Object::Name(self.name.clone().into_bytes()),
            "Encoding" => "Identity-H", "DescendantFonts" => vec![cid.into()], "ToUnicode" => to_unicode,
        })
    }

    /// The face with every unused glyph outline removed (glyph numbers stay the same, so
    /// the PDF can point at them directly). Hinting and metrics tables are kept.
    fn subset(&self) -> Option<Vec<u8>> {
        let d = &self.data;
        let tables = directory(d, self.sfnt)?;
        let (head, head_len) = *tables.get(b"head")?;
        let (loca, _) = *tables.get(b"loca")?;
        let (glyf, glyf_len) = *tables.get(b"glyf")?;
        let (maxp, _) = *tables.get(b"maxp")?;
        let count = be16(d, maxp + 4)? as usize;
        let long = be16(d, head + 50)? == 1;
        let offset = |g: usize| -> Option<usize> { if long { be32(d, loca + g * 4).map(|v| v as usize) } else { be16(d, loca + g * 2).map(|v| v as usize * 2) } };
        let glyph = |g: usize| -> Option<&[u8]> {
            let (a, b) = (offset(g)?, offset(g + 1)?);
            d.get(glyf + a..glyf + b.max(a)).filter(|_| b <= glyf_len)
        };
        // The glyphs used, plus .notdef and the parts of composite glyphs.
        let mut keep: BTreeSet<usize> = self.used.keys().map(|g| *g as usize).chain([0]).filter(|g| *g < count).collect();
        let mut todo: Vec<usize> = keep.iter().copied().collect();
        while let Some(g) = todo.pop() {
            let Some(data) = glyph(g) else { continue };
            if data.len() < 10 || (be16(data, 0)? as i16) >= 0 {
                continue;
            }
            let mut p = 10;
            loop {
                let (flags, part) = (be16(data, p)?, be16(data, p + 2)? as usize);
                if part < count && keep.insert(part) {
                    todo.push(part);
                }
                p += 4 + if flags & 1 != 0 { 4 } else { 2 };
                p += if flags & 0x8 != 0 { 2 } else if flags & 0x40 != 0 { 4 } else if flags & 0x80 != 0 { 8 } else { 0 };
                if flags & 0x20 == 0 {
                    break;
                }
            }
        }
        let mut new_glyf = Vec::new();
        let mut new_loca = Vec::with_capacity((count + 1) * 4);
        for g in 0..count {
            new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
            if keep.contains(&g) {
                new_glyf.extend_from_slice(glyph(g).unwrap_or_default());
                while new_glyf.len() % 4 != 0 {
                    new_glyf.push(0);
                }
            }
        }
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        let mut new_head = d.get(head..head + head_len)?.to_vec();
        new_head[8..12].copy_from_slice(&[0; 4]); // checkSumAdjustment, set below
        new_head[50..52].copy_from_slice(&1u16.to_be_bytes()); // long loca offsets

        let mut out: Vec<([u8; 4], Vec<u8>)> = Vec::new();
        for tag in [b"cvt ", b"fpgm", b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp", b"prep"] {
            let body = match tag {
                b"glyf" => new_glyf.clone(),
                b"loca" => new_loca.clone(),
                b"head" => new_head.clone(),
                _ => match tables.get(tag) {
                    Some((o, l)) => d.get(*o..*o + *l)?.to_vec(),
                    None => continue,
                },
            };
            out.push((*tag, body));
        }
        let n = out.len() as u16;
        let mut sel = 0u16;
        while (2u16 << sel) <= n {
            sel += 1;
        }
        let range = (1u16 << sel) * 16;
        let mut file = Vec::new();
        file.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        for v in [n, range, sel, n * 16 - range] {
            file.extend_from_slice(&v.to_be_bytes());
        }
        let mut at = 12 + 16 * out.len();
        let mut bodies = Vec::new();
        let mut head_at = 0;
        for (tag, body) in &out {
            if tag == b"head" {
                head_at = at;
            }
            file.extend_from_slice(tag);
            file.extend_from_slice(&checksum(body).to_be_bytes());
            file.extend_from_slice(&(at as u32).to_be_bytes());
            file.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let mut b = body.clone();
            while b.len() % 4 != 0 {
                b.push(0);
            }
            at += b.len();
            bodies.extend_from_slice(&b);
        }
        file.extend_from_slice(&bodies);
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&file));
        file[head_at + 8..head_at + 12].copy_from_slice(&adjust.to_be_bytes());
        Some(file)
    }
}

/// A glyph in a TJ array, with its spacing adjustments.
fn tj_item(g: &Placed) -> String {
    let mut s = String::new();
    if g.before.abs() >= 0.5 {
        s.push_str(&format!(" {:.0} ", g.before));
    }
    s.push_str(&format!("<{:04X}>", g.gid));
    if g.after.abs() >= 0.5 {
        s.push_str(&format!(" {:.0} ", g.after));
    }
    s
}

/// Table tag → (offset, length) for the face whose directory starts at `at`.
fn directory(d: &[u8], at: usize) -> Option<BTreeMap<[u8; 4], (usize, usize)>> {
    let n = be16(d, at + 4)? as usize;
    let mut map = BTreeMap::new();
    for i in 0..n {
        let r = at + 12 + i * 16;
        let tag: [u8; 4] = d.get(r..r + 4)?.try_into().ok()?;
        map.insert(tag, (be32(d, r + 8)? as usize, be32(d, r + 12)? as usize));
    }
    Some(map)
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn checksum(b: &[u8]) -> u32 {
    b.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// A short tag for the subset name (PDF wants six capitals, then "+").
fn crc(b: &[u8]) -> u32 {
    b.iter().fold(0x811C_9DC5u32, |h, x| (h ^ *x as u32).wrapping_mul(0x0100_0193))
}
