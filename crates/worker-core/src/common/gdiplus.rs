//! Windows metafiles (EMF, EMF+, WMF) drawn by GDI+, the engine Windows itself uses
//! for them, so they look as they do in Office.

use contract::{codes, FileError};
use image::RgbaImage;
use serde_json::json;
use std::path::Path;
use std::sync::OnceLock;
use windows::core::HSTRING;
use windows::Win32::Graphics::GdiPlus::*;

/// Premultiplied 32-bit BGRA (GDI+'s PixelFormat32bppPARGB).
const PARGB: i32 = 0x000E_200B;

fn start() -> Result<(), FileError> {
    static TOKEN: OnceLock<Option<usize>> = OnceLock::new();
    let ok = TOKEN.get_or_init(|| {
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        let mut token = 0usize;
        let st = unsafe { GdiplusStartup(&mut token, &input, std::ptr::null_mut()) };
        (st == Status(0)).then_some(token)
    });
    ok.map(|_| ()).ok_or_else(|| FileError::with(codes::ENGINE_FAILED, json!({ "component": "gdiplus" })))
}

/// An open metafile, closed when dropped.
struct Metafile(*mut GpMetafile);

impl Drop for Metafile {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.0 as *mut GpImage);
        }
    }
}

fn open(path: &Path) -> Result<Metafile, FileError> {
    start()?;
    let mut m: *mut GpMetafile = std::ptr::null_mut();
    let st = unsafe { GdipCreateMetafileFromFile(&HSTRING::from(path.as_os_str()), &mut m) };
    if st != Status(0) || m.is_null() {
        return Err(FileError::new(codes::CORRUPT_FILE));
    }
    Result::Ok(Metafile(m))
}

/// The drawing's size in device-independent pixels (1/96 in).
pub fn size(path: &Path) -> Result<(f64, f64), FileError> {
    let m = open(path)?;
    let (mut w, mut h) = (0f32, 0f32);
    // For metafiles the physical size is in 1/100 mm.
    let st = unsafe { GdipGetImageDimension(m.0 as *mut GpImage, &mut w, &mut h) };
    if st != Status(0) || w <= 0.0 || h <= 0.0 {
        return Err(FileError::new(codes::CORRUPT_FILE));
    }
    Result::Ok((w as f64 / 2540.0 * 96.0, h as f64 / 2540.0 * 96.0))
}

/// Draws the metafile at `w` × `h` pixels on a transparent background.
pub fn render(path: &Path, w: u32, h: u32) -> Result<RgbaImage, FileError> {
    let m = open(path)?;
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    let failed = || FileError::with(codes::ENGINE_FAILED, json!({ "component": "gdiplus" }));
    unsafe {
        let mut bmp: *mut GpBitmap = std::ptr::null_mut();
        if GdipCreateBitmapFromScan0(w as i32, h as i32, w as i32 * 4, PARGB, Some(buf.as_mut_ptr()), &mut bmp) != Status(0) || bmp.is_null() {
            return Err(failed());
        }
        let mut g: *mut GpGraphics = std::ptr::null_mut();
        let ok = GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g) == Status(0) && !g.is_null() && {
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            GdipSetTextRenderingHint(g, TextRenderingHintAntiAliasGridFit);
            GdipGraphicsClear(g, 0x0000_0000);
            let drawn = GdipDrawImageRectI(g, m.0 as *mut GpImage, 0, 0, w as i32, h as i32) == Status(0);
            GdipDeleteGraphics(g);
            drawn
        };
        GdipDisposeImage(bmp as *mut GpImage);
        if !ok {
            return Err(failed());
        }
    }
    // Premultiplied BGRA → straight RGBA.
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        let (b, g, r) = (px[0] as u32, px[1] as u32, px[2] as u32);
        let un = |c: u32| (c * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
        px[0] = un(r);
        px[1] = un(g);
        px[2] = un(b);
    }
    RgbaImage::from_raw(w, h, buf).ok_or_else(failed)
}
