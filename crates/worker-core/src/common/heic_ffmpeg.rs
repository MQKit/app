//! HEIC decoding through FFmpeg's HEVC decoder, for PCs without Microsoft's HEIF and
//! HEVC extensions. Works like libheif: the HEIF container is parsed here (`heif`),
//! the HEVC tiles become one Annex-B stream that FFmpeg decodes to RGB in a single
//! run, and the tiles are stitched and transformed (clap, irot, imir) here.

use super::heif::{self, clap_rect, Grid, Meta, Prop};
use contract::{codes, FileError};
use image::RgbImage;
use serde_json::json;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const MAX_PIXELS: u64 = 250_000_000;

fn corrupt(msg: impl Into<String>) -> FileError {
    FileError::with(codes::CORRUPT_FILE, json!({ "message": msg.into() }))
}

fn open(path: &Path) -> Result<(File, Meta), FileError> {
    let mut f = File::open(path).map_err(crate::plugins::io_err)?;
    let m = heif::read_meta(&mut f).ok_or_else(|| corrupt("no readable HEIF meta box"))?;
    Ok((f, m))
}

fn images(m: &Meta) -> Result<Vec<u32>, FileError> {
    let v = m.top_level_images();
    if v.is_empty() {
        let ty = m.item(m.primary).map(|i| String::from_utf8_lossy(&i.ty).into_owned()).unwrap_or_default();
        return Err(FileError::with(codes::UNSUPPORTED_FILE, json!({ "message": "no HEVC image", "itemType": ty })));
    }
    Ok(v)
}

fn check_size(w: u32, h: u32) -> Result<(), FileError> {
    if w == 0 || h == 0 {
        return Err(corrupt("empty image"));
    }
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(FileError::with(codes::FILE_TOO_LARGE, json!({ "width": w, "height": h })));
    }
    Ok(())
}

/// Top-level images (primary + other non-thumbnail, non-auxiliary images), for bursts.
pub fn image_count(path: &Path) -> Result<u32, FileError> {
    let (_, m) = open(path)?;
    Ok(images(&m)?.len() as u32)
}

/// (width, height) after transforms, without decoding.
pub fn dimensions(path: &Path) -> Result<(u32, u32), FileError> {
    let (_, m) = open(path)?;
    m.display_size(images(&m)?[0]).ok_or_else(|| corrupt("image size missing (ispe)"))
}

/// The ICC profile (`colr` prof) of image `index`, if any: the decoded pixels are in it.
pub fn icc(path: &Path, index: u32) -> Option<Vec<u8>> {
    let (_, m) = open(path).ok()?;
    let mut id = *images(&m).ok()?.get(index as usize)?;
    for _ in 0..4 {
        if let Some(p) = m.props_of(id).into_iter().find_map(|p| if let Prop::Icc(v) = p { Some(v.clone()) } else { None }) {
            return Some(p);
        }
        id = *m.refs(b"dimg", id).first()?;
    }
    None
}

/// Decodes image `index` (0 = primary) of a HEIC file with FFmpeg's HEVC decoder.
/// RGBA8 with the HEIF transforms applied (clap crop, irot, imir), sRGB.
pub fn decode(ffmpeg: &Path, path: &Path, index: u32) -> Result<(u32, u32, Vec<u8>), FileError> {
    decode_with(ffmpeg, path, index, &mut |_| true)
}

/// Like [`decode`], calling `progress(0..1)`; returning `false` stops FFmpeg (`cancelled`).
pub fn decode_with(ffmpeg: &Path, path: &Path, index: u32, progress: &mut dyn FnMut(f64) -> bool) -> Result<(u32, u32, Vec<u8>), FileError> {
    let (mut f, m) = open(path)?;
    let id = *images(&m)?.get(index as usize).ok_or_else(|| FileError::new(codes::RANGE_OUT_OF_BOUNDS))?;
    if let Some((w, h)) = m.display_size(id) {
        check_size(w, h)?;
    }
    let img = decode_item(ffmpeg, &m, &mut f, id, progress, 0)?;
    let img = transform(img, &m.transforms(id));
    progress(0.97);
    let (w, h) = img.dimensions();
    let rgb = img.into_raw();
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for p in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    progress(1.0);
    Ok((w, h, rgba))
}

/// The reconstructed image of item `id`, before its own transforms.
fn decode_item(ffmpeg: &Path, m: &Meta, f: &mut File, id: u32, progress: &mut dyn FnMut(f64) -> bool, depth: u32) -> Result<RgbImage, FileError> {
    let item = m.item(id).ok_or_else(|| corrupt("missing item"))?;
    match &item.ty {
        b"hvc1" => {
            let (w, h) = m.ispe(id).ok_or_else(|| corrupt("image size missing (ispe)"))?;
            check_size(w, h)?;
            let px = decode_tiles(ffmpeg, m, f, &[id], (w, h), colors_of(m, id), progress)?;
            RgbImage::from_raw(w, h, px).ok_or_else(|| corrupt("short image"))
        }
        b"grid" => {
            let desc = m.item_data(id, f).ok_or_else(|| corrupt("grid descriptor outside the file"))?;
            let g = Grid::parse(&desc).ok_or_else(|| corrupt("bad grid descriptor"))?;
            check_size(g.width, g.height)?;
            let tiles = m.refs(b"dimg", id);
            if tiles.len() != (g.rows * g.cols) as usize {
                return Err(corrupt(format!("grid {}x{} has {} tiles", g.cols, g.rows, tiles.len())));
            }
            let (tw, th) = m.ispe(tiles[0]).ok_or_else(|| corrupt("tile size missing (ispe)"))?;
            if tiles.iter().any(|&t| m.ispe(t) != Some((tw, th))) {
                return Err(corrupt("tiles of different sizes"));
            }
            if (tw as u64) * (g.cols as u64) < g.width as u64 || (th as u64) * (g.rows as u64) < g.height as u64 {
                return Err(corrupt("tiles do not cover the grid"));
            }
            check_size(tw.saturating_mul(g.cols), th.saturating_mul(g.rows))?;
            let colors = colors_of(m, id).or_else(|| colors_of(m, tiles[0]));
            let px = decode_tiles(ffmpeg, m, f, &tiles, (tw, th), colors, progress)?;
            Ok(stitch(&px, (tw, th), &g))
        }
        b"iden" if depth < 4 => {
            let src = *m.refs(b"dimg", id).first().ok_or_else(|| corrupt("iden without source"))?;
            let img = decode_item(ffmpeg, m, f, src, progress, depth + 1)?;
            Ok(transform(img, &m.transforms(src)))
        }
        other => Err(FileError::with(codes::UNSUPPORTED_FILE, json!({ "itemType": String::from_utf8_lossy(other) }))),
    }
}

/// (matrix_coefficients, full_range) from a `colr` nclx of the item.
fn colors_of(m: &Meta, id: u32) -> Option<(u16, bool)> {
    m.props_of(id).into_iter().find_map(|p| if let Prop::Nclx { matrix, full_range, .. } = p { Some((*matrix, *full_range)) } else { None })
}

/// FFmpeg `setparams` colorspace name for an ISO/IEC 23091-2 matrix.
fn colorspace_name(matrix: u16) -> Option<&'static str> {
    Some(match matrix {
        1 => "bt709",
        4 => "fcc",
        5 => "bt470bg",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "ycgco",
        9 => "bt2020nc",
        10 => "bt2020c",
        _ => return None,
    })
}

/// Length-prefixed NAL units of `tiles` → Annex-B (start codes), with the parameter
/// sets of each tile's hvcC whenever it changes.
fn annex_b(m: &Meta, f: &mut File, tiles: &[u32]) -> Result<Vec<u8>, FileError> {
    let mut out = Vec::new();
    let mut last: Option<*const Prop> = None;
    for &t in tiles {
        let cfg = m.props_of(t).into_iter().find(|p| matches!(p, Prop::Hvcc { .. })).ok_or_else(|| corrupt("image without hvcC"))?;
        let Prop::Hvcc { nals, length_size, .. } = cfg else { unreachable!() };
        if last != Some(cfg as *const Prop) {
            for n in nals {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(n);
            }
            last = Some(cfg as *const Prop);
        }
        let data = m.item_data(t, f).ok_or_else(|| corrupt("image data outside the file"))?;
        let ls = *length_size as usize;
        let mut p = 0;
        while p < data.len() {
            if p + ls > data.len() {
                return Err(corrupt("truncated NAL unit"));
            }
            let n = data[p..p + ls].iter().fold(0usize, |a, &x| (a << 8) | x as usize);
            p += ls;
            if n > data.len() - p {
                return Err(corrupt("truncated NAL unit"));
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&data[p..p + n]);
            p += n;
        }
    }
    Ok(out)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<TempDir, FileError> {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("convertor-heic-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).map_err(crate::plugins::io_err)?;
        Ok(TempDir(d))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Decodes `tiles` (each `tw`×`th`) in one FFmpeg run: consecutive RGB24 frames.
fn decode_tiles(
    ffmpeg: &Path,
    m: &Meta,
    f: &mut File,
    tiles: &[u32],
    (tw, th): (u32, u32),
    colors: Option<(u16, bool)>,
    progress: &mut dyn FnMut(f64) -> bool,
) -> Result<Vec<u8>, FileError> {
    let stream = annex_b(m, f, tiles)?;
    let tmp = TempDir::new()?;
    let input = tmp.0.join("tiles.hevc");
    std::fs::write(&input, &stream).map_err(crate::plugins::io_err)?;
    drop(stream);
    // The SPS VUI sets range and matrix; a colr nclx overrides it (HEIF), e.g. full range.
    let mut vf = String::new();
    if let Some((matrix, full)) = colors {
        vf.push_str(if full { "setparams=range=pc" } else { "setparams=range=tv" });
        if let Some(cs) = colorspace_name(matrix) {
            vf.push_str(":colorspace=");
            vf.push_str(cs);
        }
        vf.push(',');
    }
    vf.push_str("scale=flags=accurate_rnd+full_chroma_int,format=rgb24");
    let mut child = super::ffmpeg::command(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-f", "hevc", "-i"])
        .arg(&input)
        .args(["-map", "0:v:0", "-fps_mode", "passthrough", "-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "ffmpeg", "message": e.to_string() })))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let expected = tiles.len() * tw as usize * th as usize * 3;
    let mut out = Vec::with_capacity(expected);
    let mut buf = vec![0u8; 1 << 20];
    let mut fail = None;
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                fail = Some(crate::plugins::io_err(e));
                break;
            }
        };
        if out.len() + n > expected {
            fail = Some(corrupt("more pixels than expected"));
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if !progress(0.9 * out.len() as f64 / expected as f64) {
            fail = Some(FileError::new(codes::CANCELLED));
            break;
        }
    }
    if fail.is_some() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(crate::plugins::io_err)?;
    let log = err_thread.join().unwrap_or_default();
    if let Some(e) = fail {
        return Err(e);
    }
    let tail: String = log.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    if !status.success() {
        return Err(FileError::with(codes::ENGINE_FAILED, json!({ "component": "ffmpeg", "exitCode": status.code(), "log": tail })));
    }
    if out.len() != expected {
        return Err(FileError::with(
            codes::CORRUPT_FILE,
            json!({ "message": format!("FFmpeg decoded {} of {} bytes", out.len(), expected), "log": tail }),
        ));
    }
    Ok(out)
}

/// Places the tiles (row-major) on the grid canvas, cropped to the output size.
fn stitch(px: &[u8], (tw, th): (u32, u32), g: &Grid) -> RgbImage {
    let mut out = RgbImage::new(g.width, g.height);
    let (tw, th, ow, oh) = (tw as usize, th as usize, g.width as usize, g.height as usize);
    let dst = &mut *out;
    for (k, tile) in px.chunks_exact(tw * th * 3).enumerate() {
        let (x0, y0) = ((k % g.cols as usize) * tw, (k / g.cols as usize) * th);
        if x0 >= ow || y0 >= oh {
            continue;
        }
        let cw = tw.min(ow - x0) * 3;
        for y in 0..th.min(oh - y0) {
            let d = ((y0 + y) * ow + x0) * 3;
            dst[d..d + cw].copy_from_slice(&tile[y * tw * 3..y * tw * 3 + cw]);
        }
    }
    out
}

/// Applies the transformative properties (see `Meta::transforms`).
fn transform(mut img: RgbImage, props: &[&Prop]) -> RgbImage {
    use image::imageops;
    for &p in props {
        img = match p {
            Prop::Clap { .. } => match clap_rect(p, img.width(), img.height()) {
                Some((x, y, w, h)) if (w, h) != img.dimensions() => imageops::crop_imm(&img, x, y, w, h).to_image(),
                _ => img,
            },
            Prop::Irot(1) => imageops::rotate270(&img), // 90° counter-clockwise
            Prop::Irot(2) => imageops::rotate180(&img),
            Prop::Irot(3) => imageops::rotate90(&img),
            Prop::Imir(0) => imageops::flip_vertical(&img),
            Prop::Imir(_) => imageops::flip_horizontal(&img),
            _ => img,
        };
    }
    img
}

#[cfg(test)]
#[path = "heic_ffmpeg_tests.rs"]
mod tests;
