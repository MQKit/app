//! Image loading and encoding shared by the image tools.

use contract::{codes, FileError, Request};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbImage, RgbaImage};
use serde_json::{json, Map, Value};
use std::io::Cursor;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Webp,
    Gif,
    Bmp,
    Tiff,
    Ico,
    Heic,
    Avif,
    Other,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpeg",
            Kind::Png => "png",
            Kind::Webp => "webp",
            Kind::Gif => "gif",
            Kind::Bmp => "bmp",
            Kind::Tiff => "tiff",
            Kind::Ico => "ico",
            Kind::Heic => "heic",
            Kind::Avif => "avif",
            Kind::Other => "other",
        }
    }
}

/// Detects the format from the content, not from the extension.
pub fn sniff(path: &Path) -> Kind {
    let mut head = [0u8; 32];
    let n = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    let h = &head[..n];
    if h.len() >= 12 && &h[4..8] == b"ftyp" {
        let brand = &h[8..12];
        return match brand {
            b"avif" | b"avis" => Kind::Avif,
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" => Kind::Heic,
            _ => Kind::Other,
        };
    }
    match image::guess_format(h) {
        Ok(ImageFormat::Jpeg) => Kind::Jpeg,
        Ok(ImageFormat::Png) => Kind::Png,
        Ok(ImageFormat::WebP) => Kind::Webp,
        Ok(ImageFormat::Gif) => Kind::Gif,
        Ok(ImageFormat::Bmp) => Kind::Bmp,
        Ok(ImageFormat::Tiff) => Kind::Tiff,
        Ok(ImageFormat::Ico) => Kind::Ico,
        _ => Kind::Other,
    }
}

pub struct Loaded {
    pub kind: Kind,
    /// Pixels with the EXIF orientation already applied.
    pub img: DynamicImage,
    /// Raw EXIF (TIFF bytes, without the `Exif\0\0` header), orientation reset to 1.
    pub exif: Option<Vec<u8>>,
    pub icc: Option<Vec<u8>>,
}

pub fn limits(max_megapixels: u32) -> Limits {
    let mut l = Limits::default();
    let mp = max_megapixels.max(1) as u64;
    l.max_alloc = Some(mp * 1_000_000 * 8);
    l
}

pub fn load(path: &Path, req: &Request) -> Result<Loaded, FileError> {
    let max_megapixels = req.limits.max_megapixels;
    let kind = sniff(path);
    if kind == Kind::Heic {
        return load_heic(path, 0, req, &mut |_| true);
    }
    if kind == Kind::Avif {
        return load_winrt(path, kind, 0);
    }
    if kind == Kind::Other {
        return Err(FileError::new(codes::UNSUPPORTED_FILE));
    }
    // CMYK JPEGs: the image library's plain formula gives washed or too cyan colours;
    // Windows converts them through a colour profile, like print software.
    if kind == Kind::Jpeg && jpeg_components(path) == Some(4) {
        if let Ok(mut l) = load_winrt(path, kind, 0) {
            l.exif = ImageReader::open(path)
                .ok()
                .and_then(|r| r.with_guessed_format().ok())
                .and_then(|r| r.into_decoder().ok())
                .and_then(|mut d| d.exif_metadata().ok().flatten())
                .map(strip_exif_header)
                .map(|mut e| {
                    exif_reset_orientation(&mut e);
                    e
                });
            return Ok(l);
        }
    }
    let mut reader = ImageReader::open(path).map_err(crate::plugins::io_err)?.with_guessed_format().map_err(crate::plugins::io_err)?;
    reader.limits(limits(max_megapixels));
    let mut dec = match reader.into_decoder() {
        Ok(d) => d,
        // A TIFF the image library can't read: Windows' decoder (first page).
        Err(_) if kind == Kind::Tiff => return load_winrt(path, kind, 0),
        Err(e) => return Err(img_err(e)),
    };
    let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let exif = dec.exif_metadata().ok().flatten().map(strip_exif_header);
    let icc = dec.icc_profile().ok().flatten();
    let mut img = DynamicImage::from_decoder(dec).map_err(img_err)?;
    img.apply_orientation(orientation);
    let exif = exif.map(|mut e| {
        exif_reset_orientation(&mut e);
        e
    });
    Ok(Loaded { kind, img, exif, icc })
}

/// A HEIF file holding only an image sequence: a `moov` track and no `meta` (no image item).
pub fn heif_sequence_only(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let (mut meta, mut moov) = (false, false);
    let mut pos = 0u64;
    for _ in 0..64 {
        let mut h = [0u8; 16];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut h[..8]).is_err() {
            break;
        }
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        if size == 1 {
            if f.read_exact(&mut h[8..16]).is_err() {
                break;
            }
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
        }
        match &h[4..8] {
            b"meta" => meta = true,
            b"moov" => moov = true,
            _ => {}
        }
        if size < 8 {
            break;
        }
        pos += size;
    }
    moov && !meta
}

/// The number of colour components of a JPEG (1 gray, 3 colour, 4 CMYK), from its SOF.
pub fn jpeg_components(path: &Path) -> Option<u8> {
    use std::io::Read;
    let mut d = vec![0u8; 256 * 1024];
    let n = std::fs::File::open(path).ok()?.read(&mut d).ok()?;
    d.truncate(n);
    let mut p = 2;
    while p + 9 < d.len() {
        if d[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = d[p + 1];
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) || m == 0xFF {
            p += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return Some(d[p + 9]);
        }
        if m == 0xDA {
            return None;
        }
        p += 2 + u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
    }
    None
}

/// HEIC, AVIF, CMYK JPEGs and difficult TIFFs through the Windows codecs (frame `index`).
pub fn load_winrt(path: &Path, kind: Kind, index: u32) -> Result<Loaded, FileError> {
    let (w, h, rgba) = crate::common::winrt::decode_image(path, index)?;
    let img = RgbaImage::from_raw(w, h, rgba).ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
    let exif = if kind == Kind::Heic { heic_exif(path) } else { None };
    Ok(Loaded { kind, img: DynamicImage::ImageRgba8(img), exif, icc: None })
}

/// A tiny HEIC (64×64, one HEVC image) that Windows decodes only when both the HEIF
/// and the HEVC extensions are installed.
const HEIC_PROBE: &[u8] = include_bytes!("../../assets/heic-probe.heic");

/// `CONVERTOR_FORCE_HEIC_FFMPEG=1` skips the Windows codec (tests).
fn force_heic_ffmpeg() -> bool {
    std::env::var("CONVERTOR_FORCE_HEIC_FFMPEG").is_ok_and(|v| v == "1")
}

/// Whether Windows decodes HEIC on this PC. Checked once per process by decoding
/// [`HEIC_PROBE`]: with only the HEIF extension, files open but pixels do not decode.
pub fn windows_heic() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    !force_heic_ffmpeg()
        && *OK.get_or_init(|| {
            let p = std::env::temp_dir().join(format!("convertor-heic-probe-{}.heic", std::process::id()));
            let ok = std::fs::write(&p, HEIC_PROBE).is_ok()
                && crate::common::winrt::decode_image(&p, 0).is_ok_and(|(w, h, _)| (w, h) == (64, 64));
            let _ = std::fs::remove_file(&p);
            ok
        })
}

fn heic_exif(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok().and_then(|d| crate::common::heif::exif(&d)).map(|mut e| {
        exif_reset_orientation(&mut e);
        e
    })
}

/// HEIC image `index`: through Windows' codec, else through FFmpeg (`heic_ffmpeg`, the
/// engine from the request or PATH). `progress(0..1)` while FFmpeg decodes; `false` cancels.
pub fn load_heic(path: &Path, index: u32, req: &Request, progress: &mut dyn FnMut(f64) -> bool) -> Result<Loaded, FileError> {
    if windows_heic() {
        match load_winrt(path, Kind::Heic, index) {
            Err(e) if e.code == codes::ENGINE_MISSING => {}
            r => return r,
        }
    }
    let ffmpeg = crate::common::ffmpeg::exe(req, "ffmpeg")?;
    let (w, h, rgba) = crate::common::heic_ffmpeg::decode_with(&ffmpeg, path, index, progress)?;
    let img = RgbaImage::from_raw(w, h, rgba).ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
    let icc = crate::common::heic_ffmpeg::icc(path, index);
    Ok(Loaded { kind: Kind::Heic, img: DynamicImage::ImageRgba8(img), exif: heic_exif(path), icc })
}

/// Number of images in a HEIC (bursts), counted like the decoder [`load_heic`] uses.
pub fn heic_count(path: &Path) -> Result<u32, FileError> {
    if windows_heic() {
        match crate::common::winrt::image_info(path) {
            Ok(i) => return Ok(i.2),
            Err(e) if e.code != codes::ENGINE_MISSING => return Err(e),
            Err(_) => {}
        }
    }
    crate::common::heic_ffmpeg::image_count(path)
}

pub fn img_err(e: image::ImageError) -> FileError {
    match e {
        image::ImageError::Limits(_) => FileError::new(codes::FILE_TOO_LARGE),
        image::ImageError::Unsupported(u) => FileError::with(codes::UNSUPPORTED_FILE, json!({"message": u.to_string()})),
        image::ImageError::IoError(io) => crate::plugins::io_err(io),
        other => FileError::with(codes::CORRUPT_FILE, json!({"message": other.to_string()})),
    }
}

fn strip_exif_header(mut v: Vec<u8>) -> Vec<u8> {
    if v.starts_with(b"Exif\0\0") {
        v.drain(..6);
    }
    v
}

/// Fast analysis of an image: dimensions, alpha, animation, EXIF, orientation.
pub fn analyze(path: &Path) -> Result<Map<String, Value>, FileError> {
    let kind = sniff(path);
    let mut p = Map::new();
    p.insert("format".into(), json!(kind.name()));
    match kind {
        Kind::Other => {
            // A picture's name with no picture inside is a damaged file, not an unknown format.
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            const PICTURES: &[&str] = &["jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "hif", "avif", "ico"];
            let code = if PICTURES.contains(&ext.as_str()) { codes::CORRUPT_FILE } else { codes::UNSUPPORTED_FILE };
            return Err(FileError::new(code));
        }
        // A HEIF image sequence (a short animation, no still picture): not a photo to convert.
        Kind::Heic if heif_sequence_only(path) => return Err(FileError::new("heif_sequence")),
        Kind::Heic => {
            // Windows' codec when it works here, else our own HEIF parser (no decoding);
            // `heifCodec: "missing"` lets the app offer FFmpeg for decoding.
            let win = if windows_heic() {
                match crate::common::winrt::image_info(path) {
                    Ok(i) => Some(i),
                    Err(e) if e.code == codes::ENGINE_MISSING => None,
                    Err(e) => return Err(e),
                }
            } else {
                None
            };
            let (w, h, n) = match win {
                Some(i) => i,
                None => {
                    let (w, h) = crate::common::heic_ffmpeg::dimensions(path)?;
                    (w, h, crate::common::heic_ffmpeg::image_count(path)?)
                }
            };
            p.insert("width".into(), json!(w));
            p.insert("height".into(), json!(h));
            p.insert("imageCount".into(), json!(n));
            p.insert("heifCodec".into(), json!(if win.is_some() { "windows" } else { "missing" }));
            p.insert("hasExif".into(), json!(std::fs::read(path).ok().and_then(|d| crate::common::heif::exif(&d)).is_some()));
        }
        Kind::Avif => {
            let info = crate::common::winrt::image_info(path)?;
            p.insert("width".into(), json!(info.0));
            p.insert("height".into(), json!(info.1));
            p.insert("imageCount".into(), json!(info.2));
            p.insert("hasExif".into(), json!(false));
        }
        // TIFFs the image library can't read (fax G3/G4, JPEG-compressed, palette, CMYK…):
        // Windows' decoder reads them.
        Kind::Tiff if ImageReader::open(path).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.into_decoder().ok()).is_none() => {
            let info = crate::common::winrt::image_info(path).map_err(|_| FileError::new(codes::UNSUPPORTED_FILE))?;
            p.insert("width".into(), json!(info.0));
            p.insert("height".into(), json!(info.1));
            p.insert("imageCount".into(), json!(info.2));
            p.insert("hasAlpha".into(), json!(false));
            p.insert("hasExif".into(), json!(false));
            p.insert("colorModel".into(), json!("rgb"));
            p.insert("isAnimated".into(), json!(false));
        }
        _ => {
            let reader = ImageReader::open(path).map_err(crate::plugins::io_err)?.with_guessed_format().map_err(crate::plugins::io_err)?;
            let mut dec = reader.into_decoder().map_err(img_err)?;
            if kind == Kind::Tiff {
                if let Ok(info) = crate::common::winrt::image_info(path) {
                    p.insert("imageCount".into(), json!(info.2));
                }
            }
            let (mut w, mut h) = dec.dimensions();
            let o = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
            use image::metadata::Orientation::*;
            if matches!(o, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH) {
                std::mem::swap(&mut w, &mut h);
            }
            let ct = dec.color_type();
            p.insert("width".into(), json!(w));
            p.insert("height".into(), json!(h));
            p.insert("hasAlpha".into(), json!(ct.has_alpha()));
            p.insert("hasExif".into(), json!(dec.exif_metadata().ok().flatten().is_some()));
            p.insert("colorModel".into(), json!(if ct.has_color() { "rgb" } else { "gray" }));
            let apng = kind == Kind::Png && std::fs::read(path).map(|d| png_is_animated(&d)).unwrap_or(false);
            p.insert("isAnimated".into(), json!(kind == Kind::Gif && gif_frames(path) > 1 || kind == Kind::Webp && webp_animated(path) || apng));
        }
    }
    let w = p["width"].as_u64().unwrap_or(0);
    let h = p["height"].as_u64().unwrap_or(0);
    p.insert("megapixels".into(), json!(((w * h) as f64 / 1e6 * 10.0).round() / 10.0));
    p.insert("shortSide".into(), json!(w.min(h)));
    Ok(p)
}

fn gif_frames(path: &Path) -> usize {
    use image::AnimationDecoder;
    let Ok(f) = std::fs::File::open(path) else { return 0 };
    let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(f)) else { return 0 };
    dec.into_frames().take(2).count()
}

fn webp_animated(path: &Path) -> bool {
    let mut head = [0u8; 64];
    let n = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    // VP8X chunk, flags byte bit 1 = animation
    n > 21 && &head[12..16] == b"VP8X" && head[20] & 0x02 != 0
}

/// Sets the EXIF orientation tag (0x0112) in IFD0 to 1, in place.
pub fn exif_reset_orientation(tiff: &mut [u8]) {
    if tiff.len() < 8 {
        return;
    }
    let le = &tiff[0..2] == b"II";
    let rd16 = |b: &[u8], o: usize| -> u16 {
        if le {
            u16::from_le_bytes([b[o], b[o + 1]])
        } else {
            u16::from_be_bytes([b[o], b[o + 1]])
        }
    };
    let rd32 = |b: &[u8], o: usize| -> u32 {
        if le {
            u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        } else {
            u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        }
    };
    let ifd = rd32(tiff, 4) as usize;
    if ifd + 2 > tiff.len() {
        return;
    }
    let n = rd16(tiff, ifd) as usize;
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        if e + 12 > tiff.len() {
            return;
        }
        if rd16(tiff, e) == 0x0112 {
            let v = if le { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() };
            tiff[e + 8] = v[0];
            tiff[e + 9] = v[1];
            return;
        }
    }
}

/// Composites transparency over `bg` (RGB).
pub fn flatten(img: &DynamicImage, bg: [u8; 3]) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
        let a = p[3] as u32;
        for c in 0..3 {
            o[c] = ((p[c] as u32 * a + bg[c] as u32 * (255 - a) + 127) / 255) as u8;
        }
    }
    out
}

pub fn parse_color(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    let h = |i: usize| u8::from_str_radix(s.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    [h(0), h(2), h(4)]
}

/// Shrinks so the long side is at most `max` (no enlarging).
pub fn fit_long_side(img: DynamicImage, max: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if max == 0 || w.max(h) <= max {
        return img;
    }
    let s = max as f64 / w.max(h) as f64;
    let nw = ((w as f64 * s).round() as u32).max(1);
    let nh = ((h as f64 * s).round() as u32).max(1);
    resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
}

/// Resizes to exactly `w` × `h`. With transparency, the colours are weighted by alpha
/// while resampling (premultiplied): transparent pixels are often black, and mixing them
/// in gave dark halos around logos and cut-outs.
pub fn resize(src: &DynamicImage, w: u32, h: u32, filter: image::imageops::FilterType) -> DynamicImage {
    use image::{ImageBuffer, Rgba};
    if !src.color().has_alpha() {
        return src.resize_exact(w, h, filter);
    }
    let rgba = src.to_rgba8();
    // An alpha channel with nothing transparent (photos decoded by Windows): the plain way.
    if rgba.as_raw().chunks_exact(4).all(|p| p[3] == 255) {
        return src.resize_exact(w, h, filter);
    }
    // Premultiplied, 16 bits: colour × alpha (0..65025), alpha × 257.
    let pm: Vec<u16> = rgba
        .as_raw()
        .chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u16;
            [p[0] as u16 * a, p[1] as u16 * a, p[2] as u16 * a, a * 257]
        })
        .collect();
    let pm: ImageBuffer<Rgba<u16>, Vec<u16>> = ImageBuffer::from_raw(rgba.width(), rgba.height(), pm).expect("size");
    let scaled = image::imageops::resize(&pm, w, h, filter);
    let raw: Vec<u8> = scaled
        .as_raw()
        .chunks_exact(4)
        .flat_map(|p| {
            let a16 = p[3] as f32;
            if a16 < 1.0 {
                return [0, 0, 0, 0];
            }
            let c = |v: u16| ((v as f32 * 257.0 / a16).round()).clamp(0.0, 255.0) as u8;
            [c(p[0]), c(p[1]), c(p[2]), ((a16 / 257.0).round()).clamp(0.0, 255.0) as u8]
        })
        .collect();
    let out = DynamicImage::ImageRgba8(RgbaImage::from_raw(w, h, raw).expect("size"));
    if src.color().has_color() { out } else { DynamicImage::ImageLumaA8(out.to_luma_alpha8()) }
}

/// JPEG with mozjpeg (progressive, optimized), optional EXIF and ICC.
pub fn encode_jpeg(img: &DynamicImage, quality: u8, exif: Option<&[u8]>, icc: Option<&[u8]>, bg: [u8; 3]) -> Result<Vec<u8>, FileError> {
    encode_jpeg_p(img, quality, exif, icc, bg, &|_| {})
}

/// Like [`encode_jpeg`], calling `progress(0..1)` while the scanlines are written.
pub fn encode_jpeg_p(
    img: &DynamicImage,
    quality: u8,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
    bg: [u8; 3],
    progress: &(dyn Fn(f64) + Sync),
) -> Result<Vec<u8>, FileError> {
    let gray = !img.color().has_color();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<u8> = if gray { img.to_luma8().into_raw() } else { flatten(img, bg).into_raw() };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> std::io::Result<Vec<u8>> {
        let cs = if gray { mozjpeg::ColorSpace::JCS_GRAYSCALE } else { mozjpeg::ColorSpace::JCS_RGB };
        let mut c = mozjpeg::Compress::new(cs);
        c.set_size(w, h);
        c.set_quality(quality.clamp(1, 100) as f32);
        c.set_progressive_mode();
        c.set_optimize_scans(true);
        c.set_optimize_coding(true);
        let mut s = c.start_compress(Vec::new())?;
        if let Some(e) = exif {
            let mut payload = b"Exif\0\0".to_vec();
            payload.extend_from_slice(e);
            if payload.len() < 65533 {
                s.write_marker(mozjpeg::Marker::APP(1), &payload);
            }
        }
        // A profile only for the colours it describes: a CMYK one (from a CMYK JPEG decoded
        // to RGB) would make every colour wrong in programs that read profiles.
        if let Some(p) = icc.filter(|p| icc_fits(p, !gray)) {
            s.write_icc_profile(p);
        }
        let row = w * if gray { 1 } else { 3 };
        let chunk_rows = (h / 20).max(16);
        let mut y = 0;
        while y < h {
            let end = (y + chunk_rows).min(h);
            s.write_scanlines(&pixels[y * row..end * row])?;
            y = end;
            progress(y as f64 / h as f64);
        }
        s.finish()
    }))
    .map_err(|_| FileError::engine("mozjpeg failed"))?
    .map_err(FileError::from)
}

/// Whether an ICC profile describes RGB (`color`) or gray pixels: the colour space
/// signature at bytes 16..20.
pub fn icc_fits(icc: &[u8], color: bool) -> bool {
    icc.len() > 128 && icc.get(16..20) == Some(if color { b"RGB ".as_slice() } else { b"GRAY".as_slice() })
}

/// Adds an ICC profile (iCCP chunk, after IHDR) to a PNG that has none, when it fits the
/// PNG's colours (gray for gray PNGs; RGB for colour and palette ones).
pub fn png_with_icc(png: Vec<u8>, icc: &[u8]) -> Vec<u8> {
    if png.len() < 33 || &png[12..16] != b"IHDR" {
        return png;
    }
    let color = matches!(png[25], 2 | 3 | 6);
    if !icc_fits(icc, color) || png.windows(4).any(|w| w == b"iCCP" || w == b"sRGB") {
        return png;
    }
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    if z.write_all(icc).is_err() {
        return png;
    }
    let Ok(packed) = z.finish() else { return png };
    let mut data = b"ICC profile\0\0".to_vec();
    data.extend_from_slice(&packed);
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = b"iCCP".to_vec();
    body.extend_from_slice(&data);
    chunk.extend_from_slice(&body);
    let mut crc = flate2::Crc::new();
    crc.update(&body);
    chunk.extend_from_slice(&crc.sum().to_be_bytes());
    let mut out = Vec::with_capacity(png.len() + chunk.len());
    out.extend_from_slice(&png[..33]);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&png[33..]);
    out
}

/// Whether a PNG is animated (an APNG: it has an acTL chunk before the image data).
pub fn png_is_animated(data: &[u8]) -> bool {
    let mut p = 8;
    while p + 8 <= data.len() {
        let len = u32::from_be_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]) as usize;
        let ty = &data[p + 4..p + 8];
        if ty == b"acTL" {
            return true;
        }
        if ty == b"IDAT" {
            return false;
        }
        p += 12 + len;
    }
    false
}

/// PNG: lossless (oxipng) or lossy (imagequant palette + oxipng).
pub fn encode_png(img: &DynamicImage, lossy: bool, quality: u8) -> Result<Vec<u8>, FileError> {
    let raw = if lossy {
        quantize_png(img, quality)?
    } else {
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png).map_err(img_err)?;
        buf
    };
    let mut opts = oxipng::Options::from_preset(2);
    opts.strip = oxipng::StripChunks::Safe;
    oxipng::optimize_from_memory(&raw, &opts).map_err(|e| FileError::engine(e.to_string()))
}

/// A PNG written straight away, without the optimizer: for big photos (seconds, not
/// tens of seconds), where squeezing the last few percent isn't worth the wait.
pub fn encode_png_quick(img: &DynamicImage) -> Result<Vec<u8>, FileError> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut buf = Vec::new();
    img.write_with_encoder(PngEncoder::new_with_quality(&mut buf, CompressionType::Default, FilterType::Adaptive)).map_err(img_err)?;
    Ok(buf)
}

fn quantize_png(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, FileError> {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let px: Vec<imagequant::RGBA> = rgba.pixels().map(|p| imagequant::RGBA::new(p[0], p[1], p[2], p[3])).collect();
    let mut liq = imagequant::new();
    liq.set_speed(4).map_err(|e| FileError::engine(e.to_string()))?;
    liq.set_quality(0, quality.clamp(1, 100)).map_err(|e| FileError::engine(e.to_string()))?;
    let mut qimg = liq.new_image(px, w, h, 0.0).map_err(|e| FileError::engine(e.to_string()))?;
    let mut res = liq.quantize(&mut qimg).map_err(|e| FileError::engine(e.to_string()))?;
    res.set_dithering_level(1.0).map_err(|e| FileError::engine(e.to_string()))?;
    let (palette, indexes) = res.remapped(&mut qimg).map_err(|e| FileError::engine(e.to_string()))?;
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w as u32, h as u32);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect::<Vec<u8>>());
        if palette.iter().any(|c| c.a < 255) {
            enc.set_trns(palette.iter().map(|c| c.a).collect::<Vec<u8>>());
        }
        let mut wr = enc.write_header().map_err(|e| FileError::engine(e.to_string()))?;
        wr.write_image_data(&indexes).map_err(|e| FileError::engine(e.to_string()))?;
    }
    Ok(buf)
}

pub fn encode_webp(img: &DynamicImage, quality: u8, lossless: bool) -> Result<Vec<u8>, FileError> {
    let (w, h) = (img.width(), img.height());
    let enc_data = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let enc = webp::Encoder::from_rgba(rgba.as_raw(), w, h);
        if lossless { enc.encode_lossless() } else { enc.encode(quality.clamp(1, 100) as f32) }
    } else {
        let rgb = img.to_rgb8();
        let enc = webp::Encoder::from_rgb(rgb.as_raw(), w, h);
        if lossless { enc.encode_lossless() } else { enc.encode(quality.clamp(1, 100) as f32) }
    };
    Ok(enc_data.to_vec())
}

/// Other formats through the `image` crate.
/// TIFF compressed with LZW and the horizontal predictor (what every reader opens): gray
/// stays gray, 16 bits stay 16 bits, transparency is kept.
pub fn encode_tiff(img: &DynamicImage) -> Result<Vec<u8>, FileError> {
    use tiff::encoder::{colortype, Compression, Predictor, TiffEncoder};
    let mut buf = Cursor::new(Vec::new());
    let err = |e: tiff::TiffError| FileError::engine(e.to_string());
    {
        let mut enc = TiffEncoder::new(&mut buf).map_err(err)?.with_compression(Compression::Lzw).with_predictor(Predictor::Horizontal);
        let (w, h) = (img.width(), img.height());
        let wide = matches!(img.color(), image::ColorType::L16 | image::ColorType::La16 | image::ColorType::Rgb16 | image::ColorType::Rgba16);
        let alpha = img.color().has_alpha();
        match (img.color().has_color(), alpha, wide) {
            (false, false, false) => enc.write_image::<colortype::Gray8>(w, h, img.to_luma8().as_raw()),
            (false, false, true) => enc.write_image::<colortype::Gray16>(w, h, img.to_luma16().as_raw()),
            (_, true, false) => enc.write_image::<colortype::RGBA8>(w, h, img.to_rgba8().as_raw()),
            (_, true, true) => enc.write_image::<colortype::RGBA16>(w, h, img.to_rgba16().as_raw()),
            (true, false, false) => enc.write_image::<colortype::RGB8>(w, h, img.to_rgb8().as_raw()),
            (true, false, true) => enc.write_image::<colortype::RGB16>(w, h, img.to_rgb16().as_raw()),
        }
        .map_err(err)?;
    }
    Ok(buf.into_inner())
}

pub fn encode_with(img: &DynamicImage, fmt: ImageFormat, bg: [u8; 3]) -> Result<Vec<u8>, FileError> {
    let mut buf = Vec::new();
    let img = match fmt {
        ImageFormat::Bmp => DynamicImage::ImageRgb8(flatten(img, bg)),
        ImageFormat::Ico => {
            let i = fit_long_side(img.clone(), 256);
            DynamicImage::ImageRgba8(i.to_rgba8())
        }
        ImageFormat::Gif => DynamicImage::ImageRgba8(img.to_rgba8()),
        ImageFormat::Tiff => return encode_tiff(img),
        _ => img.clone(),
    };
    img.write_to(&mut Cursor::new(&mut buf), fmt).map_err(img_err)?;
    Ok(buf)
}

/// Finds the highest quality whose output fits in `target` bytes (binary search).
/// Returns (bytes, quality) or the smallest attempt when nothing fits.
pub fn fit_quality(
    target: u64,
    min_q: u8,
    max_q: u8,
    mut encode: impl FnMut(u8) -> Result<Vec<u8>, FileError>,
) -> Result<(Vec<u8>, u8, bool), FileError> {
    let (mut lo, mut hi) = (min_q, max_q);
    let mut best: Option<(Vec<u8>, u8)> = None;
    let mut smallest: Option<(Vec<u8>, u8)> = None;
    for _ in 0..8 {
        if lo > hi {
            break;
        }
        let q = lo + (hi - lo) / 2;
        let data = encode(q)?;
        let fits = data.len() as u64 <= target;
        if smallest.as_ref().map(|s| data.len() < s.0.len()).unwrap_or(true) {
            smallest = Some((data.clone(), q));
        }
        if fits {
            best = Some((data, q));
            lo = q + 1;
        } else {
            if q == 0 {
                break;
            }
            hi = q - 1;
        }
    }
    match best {
        Some((d, q)) => Ok((d, q, true)),
        None => {
            let (d, q) = smallest.ok_or_else(|| FileError::new(codes::TARGET_UNREACHABLE))?;
            Ok((d, q, false))
        }
    }
}


/// A simple WebP (VP8/VP8L) wrapped in the extended format with EXIF and/or an ICC profile.
pub fn webp_with_meta(data: Vec<u8>, exif: Option<&[u8]>, icc: Option<&[u8]>, w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let icc = icc.filter(|p| icc_fits(p, true));
    if exif.is_none() && icc.is_none() {
        return data;
    }
    if data.len() < 20 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return data;
    }
    // Already extended (transparency adds an ALPH chunk): set the flags and add the chunks.
    if &data[12..16] == b"VP8X" && data.len() >= 30 {
        let chunk = |tag: &[u8], body: &[u8]| -> Vec<u8> {
            let mut c = tag.to_vec();
            c.extend_from_slice(&(body.len() as u32).to_le_bytes());
            c.extend_from_slice(body);
            if body.len() % 2 == 1 {
                c.push(0);
            }
            c
        };
        let has = |tag: &[u8]| data.windows(4).any(|w| w == tag);
        let mut out = data[..30].to_vec();
        if let Some(p) = icc.filter(|_| !has(b"ICCP")) {
            out[20] |= 0x20;
            out.extend(chunk(b"ICCP", p));
        }
        out.extend_from_slice(&data[30..]);
        if let Some(e) = exif.filter(|_| !has(b"EXIF")) {
            out[20] |= 0x08;
            out.extend(chunk(b"EXIF", e));
        }
        let size = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&size.to_le_bytes());
        return out;
    }
    if !(&data[12..16] == b"VP8 " || &data[12..16] == b"VP8L") {
        return data;
    }
    let image_chunk = &data[12..];
    let mut vp8x = Vec::with_capacity(18);
    vp8x.extend_from_slice(b"VP8X");
    vp8x.extend_from_slice(&10u32.to_le_bytes());
    let flags: u8 = if exif.is_some() { 0x08 } else { 0 } | if icc.is_some() { 0x20 } else { 0 } | if alpha { 0x10 } else { 0 };
    vp8x.extend_from_slice(&[flags, 0, 0, 0]);
    let (cw, ch) = (w.saturating_sub(1), h.saturating_sub(1));
    vp8x.extend_from_slice(&cw.to_le_bytes()[..3]);
    vp8x.extend_from_slice(&ch.to_le_bytes()[..3]);
    let chunk = |tag: &[u8], body: &[u8]| -> Vec<u8> {
        let mut c = Vec::with_capacity(body.len() + 9);
        c.extend_from_slice(tag);
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    // Order: VP8X, ICCP, the image, EXIF.
    let iccp = icc.map(|p| chunk(b"ICCP", p)).unwrap_or_default();
    let exif_chunk = exif.map(|e| chunk(b"EXIF", e)).unwrap_or_default();
    let body_len = 4 + vp8x.len() + iccp.len() + image_chunk.len() + exif_chunk.len();
    let mut out = Vec::with_capacity(body_len + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body_len as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&vp8x);
    out.extend_from_slice(&iccp);
    out.extend_from_slice(image_chunk);
    out.extend_from_slice(&exif_chunk);
    out
}

/// All frames (composited, RGBA) and their delays in ms, for animated GIF and WebP.
pub fn animation_frames(path: &Path) -> Option<Vec<(image::RgbaImage, u32)>> {
    use image::AnimationDecoder;
    let f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let frames: Vec<image::Frame> = match sniff(path) {
        Kind::Gif => image::codecs::gif::GifDecoder::new(f).ok()?.into_frames().collect_frames().ok()?,
        Kind::Webp => image::codecs::webp::WebPDecoder::new(f).ok()?.into_frames().collect_frames().ok()?,
        Kind::Png => image::codecs::png::PngDecoder::new(f).ok()?.apng().ok()?.into_frames().collect_frames().ok()?,
        _ => return None,
    };
    if frames.len() < 2 {
        return None;
    }
    Some(
        frames
            .into_iter()
            .map(|fr| {
                let (n, d) = fr.delay().numer_denom_ms();
                let ms = n.checked_div(d).map_or(100, |v| v.max(20));
                (fr.into_buffer(), ms)
            })
            .collect(),
    )
}

pub fn encode_gif_anim(frames: &[(image::RgbaImage, u32)]) -> Result<Vec<u8>, FileError> {
    let mut buf = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut buf, 10);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(img_err)?;
        for (im, ms) in frames {
            let fr = image::Frame::from_parts(im.clone(), 0, 0, image::Delay::from_numer_denom_ms(*ms, 1));
            enc.encode_frame(fr).map_err(img_err)?;
        }
    }
    Ok(buf)
}

pub fn encode_webp_anim(frames: &[(image::RgbaImage, u32)], quality: u8, lossless: bool) -> Result<Vec<u8>, FileError> {
    let (w, h) = frames.first().map(|f| f.0.dimensions()).ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
    let mut config = webp::WebPConfig::new().map_err(|_| FileError::engine("webp config"))?;
    config.quality = quality.clamp(1, 100) as f32;
    config.lossless = if lossless { 1 } else { 0 };
    let mut enc = webp::AnimEncoder::new(w, h, &config);
    enc.set_loop_count(0);
    let mut t = 0i32;
    for (im, ms) in frames {
        enc.add_frame(webp::AnimFrame::from_rgba(im.as_raw(), w, h, t));
        t += *ms as i32;
    }
    let data = enc.try_encode().map_err(|e| FileError::engine(format!("{e:?}")))?;
    Ok(data.to_vec())
}
