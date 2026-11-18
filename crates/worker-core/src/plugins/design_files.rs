//! Design files: Photoshop (PSD, PSB), Illustrator (AI) and EPS to PNG, JPG, WebP or TIFF,
//! without those programs.
//! - PSD/PSB: the flattened picture Photoshop keeps next to the layers (8, 16 or 32 bits;
//!   RGB, greyscale or CMYK; transparency), read here.
//! - AI: the PDF inside, drawn by Windows.
//! - EPS: with Ghostscript when it is installed; else the preview picture inside the file.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::img;
use contract::{opt, t, FileError, FileRef, Request};
use image::{DynamicImage, GrayAlphaImage, RgbaImage};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const DESIGN: &[&str] = &["psd", "psb", "ai", "eps", "epsf", "ps"];

pub struct DesignFiles;

impl Plugin for DesignFiles {
    fn id(&self) -> &'static str {
        "design-files"
    }

    fn manifest(&self) -> Value {
        let vector = json!({ "file": "vector", "eq": true, "quant": "any" });
        json!({
            "name": t("PSD și fișiere de design", "PSD and design files"),
            "summary": t("Fără Photoshop sau Illustrator", "Without Photoshop or Illustrator"),
            "keywords": { "ro": ["psd în png", "psd în jpg", "photoshop", "illustrator", "ai în png", "eps în png", "psb"],
                          "en": ["psd to png", "psd to jpg", "photoshop", "illustrator", "ai to png", "eps to png", "psb"] },
            "category": "image",
            "icon": "layers-intersect",
            "input": { "extensions": DESIGN, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["png", "jpg", "webp", "tif"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["kind", "vector", "width", "height", "epsPreviewOnly"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "png", "display": "chips",
                  "options": [ opt("png", "PNG", "PNG"), opt("jpg", "JPG", "JPG"), opt("webp", "WebP", "WebP"), opt("tif", "TIFF", "TIFF") ],
                  "help": t("PNG, WebP și TIFF păstrează transparența.", "PNG, WebP and TIFF keep the transparency.") },
                { "key": "dpi", "type": "select", "label": t("Rezoluție (AI, EPS)", "Resolution (AI, EPS)"), "default": 300, "showIf": vector,
                  "options": [ opt(150, "150 dpi (ecran)", "150 dpi (screen)"), opt(300, "300 dpi (tipar)", "300 dpi (print)"), opt(600, "600 dpi", "600 dpi") ] }
            ],
            "rules": [
                { "id": "eps_preview", "severity": "warning", "when": { "file": "epsPreviewOnly", "eq": true, "quant": "any" },
                  "message": t("Pentru EPS se folosește poza de previzualizare din fișier (rezoluție mică). Pentru calitate întreagă, instalează Ghostscript, gratuit, de pe ghostscript.com.",
                               "For EPS the preview picture inside the file is used (low resolution). For full quality, install Ghostscript, free, from ghostscript.com.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "psd_unreadable": t("Fișierul Photoshop nu poate fi citit: {message}", "The Photoshop file can't be read: {message}"),
                "eps_no_preview": t("EPS-ul nu are poză de previzualizare. Instalează Ghostscript (gratuit) și încearcă din nou.",
                                    "The EPS has no preview picture. Install Ghostscript (free) and try again."),
                "ai_not_pdf": t("Fișierul Illustrator a fost salvat fără „Create PDF Compatible File”; deschide-l în Illustrator și salvează-l cu această opțiune.",
                                "The Illustrator file was saved without “Create PDF Compatible File”; open it in Illustrator and save it with that option."),
                "timeout": t("Desenul din fișier nu se mai termina și a fost oprit.", "The drawing in the file never finished and was stopped.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let path = f.input_path();
        let ext = ext_of(path);
        let mut p = Map::new();
        p.insert("kind".into(), json!(ext));
        p.insert("vector".into(), json!(matches!(ext.as_str(), "ai" | "eps" | "epsf" | "ps")));
        p.insert("epsPreviewOnly".into(), json!(matches!(ext.as_str(), "eps" | "epsf" | "ps") && ghostscript().is_none()));
        if matches!(ext.as_str(), "psd" | "psb") {
            match psd_header(path) {
                Some(h) => {
                    p.insert("width".into(), json!(h.width));
                    p.insert("height".into(), json!(h.height));
                }
                None => return Analysis::status("unsupported", "psd_unreadable"),
            }
        }
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let dpi = req.f64_or("dpi", 300.0).clamp(36.0, 1200.0);
        let im = match ext_of(path).as_str() {
            "psd" | "psb" => psd_composite(&std::fs::read(path).map_err(super::io_err)?)?,
            "ai" => {
                let pdf = crate::common::winrt::Pdf::open(path, None).map_err(|_| FileError::new("ai_not_pdf"))?;
                let png = pdf.render_png(0, dpi, true)?;
                image::load_from_memory(&png).map_err(img::img_err)?
            }
            _ => eps(ctx, &f.id, path, dpi)?,
        };
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.7, "save", 2, 2);
        let format = req.str_or("format", "png");
        let bytes = match format {
            "jpg" => img::encode_jpeg(&im, 92, None, None, [255, 255, 255])?,
            "webp" => img::encode_webp(&DynamicImage::ImageRgba8(im.to_rgba8()), 92, false)?,
            "tif" => {
                let mut buf = Vec::new();
                let out = if im.color().has_alpha() { DynamicImage::ImageRgba16(im.to_rgba16()) } else { DynamicImage::ImageRgb16(im.to_rgb16()) };
                out.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Tiff).map_err(img::img_err)?;
                buf
            }
            _ => img::encode_png_quick(&im)?,
        };
        let dest = ctx.file_dir(&f.id)?.join(format!("result.{format}"));
        write_file(&dest, &bytes)?;
        ctx.progress(&f.id, 1.0, "save", 2, 2);
        ctx.emit_output(&f.id, &dest, format, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

// ------------------------------------------------------------------ PSD / PSB

struct PsdHeader {
    big: bool,
    channels: usize,
    height: u32,
    width: u32,
    depth: u16,
    mode: u16,
}

fn psd_header(path: &Path) -> Option<PsdHeader> {
    let mut h = [0u8; 26];
    std::fs::File::open(path).ok()?.read_exact(&mut h).ok()?;
    parse_header(&h)
}

fn parse_header(h: &[u8]) -> Option<PsdHeader> {
    if h.get(..4)? != b"8BPS" {
        return None;
    }
    let version = u16::from_be_bytes([h[4], h[5]]);
    Some(PsdHeader {
        big: version == 2,
        channels: u16::from_be_bytes([h[12], h[13]]) as usize,
        height: u32::from_be_bytes([h[14], h[15], h[16], h[17]]),
        width: u32::from_be_bytes([h[18], h[19], h[20], h[21]]),
        depth: u16::from_be_bytes([h[22], h[23]]),
        mode: u16::from_be_bytes([h[24], h[25]]),
    })
}

fn bad(msg: &str) -> FileError {
    FileError::with("psd_unreadable", json!({ "message": msg }))
}

struct Cursor<'a> {
    d: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], FileError> {
        let s = self.d.get(self.i..self.i.checked_add(n).ok_or_else(|| bad("size"))?).ok_or_else(|| bad("the file ends too soon"))?;
        self.i += n;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, FileError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, FileError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, FileError> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap()))
    }
}

/// PackBits: a run length byte n; 0…127 → n+1 literal bytes, 129…255 → one byte 257-n times.
fn unpack_bits(src: &[u8], out: &mut Vec<u8>, want: usize) {
    let mut i = 0;
    while i < src.len() && out.len() < want {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = n as usize + 1;
            out.extend_from_slice(&src[i..(i + len).min(src.len())]);
            i += len;
        } else if n != -128 {
            let len = (1 - n as isize) as usize;
            if let Some(&b) = src.get(i) {
                out.extend(std::iter::repeat_n(b, len));
            }
            i += 1;
        }
    }
    out.resize(want, 0);
}

/// The colour of each extra channel that is a spot ink (None for the others), from the
/// image resources: DisplayInfo (0x0435), or the older 0x03EF.
fn spot_inks(res: &[u8]) -> Vec<Option<[u8; 3]>> {
    let mut i = 0;
    let mut found: Option<(u16, &[u8])> = None;
    while i + 12 <= res.len() && &res[i..i + 4] == b"8BIM" {
        let id = u16::from_be_bytes([res[i + 4], res[i + 5]]);
        let name = (res[i + 6] as usize + 2) & !1;
        let at = i + 6 + name;
        let Some(size) = res.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize) else { break };
        let data = res.get(at + 4..at + 4 + size).unwrap_or(&[]);
        if id == 0x0435 || (id == 0x03EF && found.is_none()) {
            found = Some((id, data));
        }
        i = at + 4 + size + (size & 1);
    }
    let Some((id, data)) = found else { return vec![] };
    // 0x0435 starts with a version; each channel: space, four components, opacity, kind (1 byte).
    let (start, step) = if id == 0x0435 { (4, 13) } else { (0, 14) };
    let mut out = Vec::new();
    let mut k = start;
    while k + step <= data.len() {
        let e = &data[k..k + step];
        let space = u16::from_be_bytes([e[0], e[1]]);
        let c = |n: usize| (u16::from_be_bytes([e[2 + 2 * n], e[3 + 2 * n]]) >> 8) as u8;
        let kind = e[12];
        let rgb = match space {
            0 => Some([c(0), c(1), c(2)]),
            // CMYK, 0 = full ink.
            2 => {
                let k = 255 - c(3) as u32;
                Some([0, 1, 2].map(|n| ((c(n) as u32 * (255 - k)) / 255) as u8))
            }
            _ => None,
        };
        out.push(if kind == 2 { rgb } else { None });
        k += step;
    }
    out
}

/// The merged picture of a Photoshop file.
pub fn psd_composite(d: &[u8]) -> Result<DynamicImage, FileError> {
    let h = parse_header(d).ok_or_else(|| bad("not a Photoshop file"))?;
    let (w, ht) = (h.width as usize, h.height as usize);
    if w == 0 || ht == 0 || w * ht > 400_000_000 {
        return Err(bad("size"));
    }
    let mut c = Cursor { d, i: 26 };
    let color_len = c.u32()? as usize;
    c.take(color_len)?;
    let res_len = c.u32()? as usize;
    let inks = spot_inks(c.take(res_len)?);
    // Layers: skipped, except to learn whether the merged picture has transparency
    // (Photoshop writes a negative layer count then).
    let layers_len = if h.big { c.u64()? as usize } else { c.u32()? as usize };
    let layers_start = c.i;
    let mut merged_alpha = false;
    if layers_len >= 6 {
        let info_len = if h.big { c.u64()? } else { c.u32()? as u64 };
        if info_len >= 2 {
            let count = c.u16()? as i16;
            merged_alpha = count < 0;
        }
    }
    c.i = layers_start + layers_len;
    let compression = c.u16()?;
    let bytes = (h.depth as usize).div_ceil(8).max(1);
    let row = if h.depth == 1 { w.div_ceil(8) } else { w * bytes };
    let color_channels = match h.mode {
        3 => 3,
        4 => 4,
        9 => 3,
        _ => 1,
    };
    // Only the colour channels and (when there is one) transparency are needed.
    let alpha = h.channels > color_channels && (merged_alpha || (h.mode == 3 && h.channels == 4) || (h.mode == 1 && h.channels == 2));
    // Spot colour channels (printing inks: a fluorescent pink, a varnish…) are painted over
    // the colours as Photoshop shows them. A file drawn only in spot inks came out white.
    let spots: Vec<(usize, [u8; 3])> = if alpha || merged_alpha {
        vec![]
    } else {
        inks.iter().enumerate().filter_map(|(j, ink)| ink.map(|rgb| (color_channels + j, rgb))).filter(|(ch, _)| *ch < h.channels).collect()
    };
    let needed = spots.iter().map(|(ch, _)| ch + 1).max().unwrap_or(0).max(color_channels + alpha as usize);
    if h.channels < color_channels {
        return Err(bad("missing colour channels"));
    }
    let plane_len = row * ht;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(needed);
    match compression {
        0 => {
            for _ in 0..needed {
                planes.push(c.take(plane_len)?.to_vec());
            }
        }
        1 => {
            let rows = ht * h.channels;
            let mut counts = Vec::with_capacity(rows);
            for _ in 0..rows {
                counts.push(if h.big { c.u32()? as usize } else { c.u16()? as usize });
            }
            for ch in 0..needed {
                let mut plane = Vec::with_capacity(plane_len);
                for y in 0..ht {
                    let packed = c.take(counts[ch * ht + y])?;
                    let mut line = Vec::with_capacity(row);
                    unpack_bits(packed, &mut line, row);
                    plane.extend_from_slice(&line);
                }
                planes.push(plane);
            }
        }
        2 | 3 => {
            // ZIP (with prediction for 3): one zlib stream for all channels.
            // Only the channels used: a stream that unpacks to more is never read further.
            let want = needed.checked_mul(plane_len).ok_or_else(|| bad("size"))?;
            let mut all = Vec::new();
            flate2::read::ZlibDecoder::new(&d[c.i..]).take(want as u64).read_to_end(&mut all).map_err(|_| bad("zip"))?;
            for ch in 0..needed {
                let mut p = all.get(ch * plane_len..(ch + 1) * plane_len).ok_or_else(|| bad("zip"))?.to_vec();
                if compression == 3 && bytes == 1 {
                    for line in p.chunks_mut(row) {
                        for x in 1..line.len() {
                            line[x] = line[x].wrapping_add(line[x - 1]);
                        }
                    }
                }
                planes.push(p);
            }
        }
        _ => return Err(bad("unknown compression")),
    }
    // Every sample as 0…255.
    let sample = |plane: &[u8], i: usize| -> u8 {
        match h.depth {
            1 => if plane[(i / w) * row + (i % w) / 8] >> (7 - (i % w) % 8) & 1 == 1 { 0 } else { 255 },
            16 => plane[i * 2],
            32 => {
                let v = f32::from_be_bytes([plane[i * 4], plane[i * 4 + 1], plane[i * 4 + 2], plane[i * 4 + 3]]);
                // 32-bit files are linear light.
                (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
            }
            _ => plane[i],
        }
    };
    let n = w * ht;
    let a = |i: usize| if alpha { sample(&planes[color_channels], i) } else { 255 };
    let mut out = match h.mode {
        3 => {
            let mut im = RgbaImage::new(h.width, h.height);
            for (i, px) in im.pixels_mut().enumerate().take(n) {
                px.0 = [sample(&planes[0], i), sample(&planes[1], i), sample(&planes[2], i), a(i)];
            }
            DynamicImage::ImageRgba8(im)
        }
        4 => {
            // Photoshop stores CMYK inverted (255 = no ink); a plain conversion, no profile.
            let mut im = RgbaImage::new(h.width, h.height);
            for (i, px) in im.pixels_mut().enumerate().take(n) {
                let k = sample(&planes[3], i) as u32;
                let f = |v: u8| ((v as u32 * k) / 255) as u8;
                px.0 = [f(sample(&planes[0], i)), f(sample(&planes[1], i)), f(sample(&planes[2], i)), a(i)];
            }
            DynamicImage::ImageRgba8(im)
        }
        2 => {
            // Indexed: the palette is in the colour mode data (768 bytes: all R, all G, all B).
            let pal = &d[30..30 + color_len.min(768)];
            let mut im = RgbaImage::new(h.width, h.height);
            for (i, px) in im.pixels_mut().enumerate().take(n) {
                let k = planes[0][i] as usize;
                let g = |o: usize| *pal.get(o * 256 + k).unwrap_or(&0);
                px.0 = [g(0), g(1), g(2), 255];
            }
            DynamicImage::ImageRgba8(im)
        }
        9 => return Err(bad("Lab colour")),
        _ => {
            let mut im = GrayAlphaImage::new(h.width, h.height);
            for (i, px) in im.pixels_mut().enumerate().take(n) {
                px.0 = [sample(&planes[0], i), a(i)];
            }
            DynamicImage::ImageLumaA8(im)
        }
    };
    if !spots.is_empty() {
        let mut rgba = out.to_rgba8();
        for (ch, ink) in &spots {
            for (i, px) in rgba.pixels_mut().enumerate().take(n) {
                // Stored like the inks of CMYK: 255 is none, 0 is solid.
                let density = (255 - sample(&planes[*ch], i)) as u32;
                if density == 0 {
                    continue;
                }
                for (k, &c) in ink.iter().enumerate().take(3) {
                    let tint = 255 * 255 - density * (255 - c as u32);
                    px.0[k] = ((px.0[k] as u32 * tint) / (255 * 255)) as u8;
                }
            }
        }
        out = DynamicImage::ImageRgba8(rgba);
    }
    Ok(out)
}

// ------------------------------------------------------------------ EPS

/// gswin64c.exe of the newest Ghostscript installed, if any.
fn ghostscript() -> Option<PathBuf> {
    let mut found = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(var) {
            if let Ok(rd) = std::fs::read_dir(Path::new(&base).join("gs")) {
                for e in rd.flatten() {
                    for exe in ["gswin64c.exe", "gswin32c.exe"] {
                        let p = e.path().join("bin").join(exe);
                        if p.is_file() {
                            found.push(p);
                        }
                    }
                }
            }
        }
    }
    found.sort();
    found.pop()
}

fn eps(ctx: &Ctx, id: &str, path: &Path, dpi: f64) -> Result<DynamicImage, FileError> {
    if let Some(gs) = ghostscript() {
        use crate::common::proc;
        use std::time::Duration;
        let out = ctx.file_dir(id)?.join("gs.png");
        let mut cmd = std::process::Command::new(gs);
        cmd.args(["-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET", "-dEPSCrop", "-sDEVICE=pngalpha", "-dTextAlphaBits=4", "-dGraphicsAlphaBits=4"])
            .arg(format!("-r{}", dpi.round()))
            .arg(format!("-sOutputFile={}", out.display()))
            .arg(path)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000);
        let mut child = cmd.spawn().map_err(|e| FileError::engine(format!("ghostscript: {e}")))?;
        // PostScript is a program: a file can draw forever. Minutes, more for big files.
        let limit = proc::limit_for(file_size(path), Duration::from_secs(180), Duration::from_secs(10), Duration::from_secs(30 * 60));
        let status = proc::wait(&mut child, Some(&ctx.cancel), "ghostscript", limit, || {});
        let status = match status {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                return Err(e);
            }
        };
        if status.success() && out.is_file() {
            let im = image::open(&out).map_err(img::img_err)?;
            let _ = std::fs::remove_file(&out);
            return Ok(im);
        }
    }
    eps_preview(ctx, id, path)
}

/// The preview a "DOS EPS" carries: a TIFF or a WMF picture after a 30-byte header.
fn eps_preview(ctx: &Ctx, id: &str, path: &Path) -> Result<DynamicImage, FileError> {
    let d = std::fs::read(path).map_err(super::io_err)?;
    let none = || FileError::new("eps_no_preview");
    if d.get(..4) != Some(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        return Err(none());
    }
    let u = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).unwrap_or(0);
    let (wmf_at, wmf_len, tif_at, tif_len) = (u(12), u(16), u(20), u(24));
    if tif_len > 0 {
        let tif = d.get(tif_at..tif_at + tif_len).ok_or_else(none)?;
        return image::load_from_memory_with_format(tif, image::ImageFormat::Tiff).map_err(img::img_err);
    }
    if wmf_len > 0 {
        #[cfg(windows)]
        {
            let wmf = d.get(wmf_at..wmf_at + wmf_len).ok_or_else(none)?;
            let tmp = ctx.file_dir(id)?.join("preview.wmf");
            write_file(&tmp, wmf)?;
            let (w, h) = crate::common::gdiplus::size(&tmp)?;
            let im = crate::common::gdiplus::render(&tmp, (w * 2.0).round().max(1.0) as u32, (h * 2.0).round().max(1.0) as u32)?;
            let _ = std::fs::remove_file(&tmp);
            return Ok(DynamicImage::ImageRgba8(im));
        }
    }
    let _ = (ctx, id, wmf_at);
    Err(none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits() {
        let mut out = Vec::new();
        unpack_bits(&[0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xF7, 0x80], &mut out, 15);
        assert_eq!(out, vec![0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]);
    }
}
