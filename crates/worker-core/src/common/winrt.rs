//! Windows Runtime codecs: HEIC/AVIF decoding (Windows.Graphics.Imaging)
//! and PDF page rendering (Windows.Data.Pdf). No extra DLLs to ship.

use contract::{codes, FileError};
use serde_json::json;
use std::path::Path;
use std::sync::Once;
use windows::core::HSTRING;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapEncoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode, ExifOrientationMode,
};
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream, IRandomAccessStream};
use windows::Storage::FileAccessMode;

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    });
}

fn win_err(e: windows::core::Error) -> FileError {
    let hr = e.code().0 as u32;
    let code = match hr {
        0x80070002 | 0x80070003 => codes::UNSUPPORTED_FILE,
        0x80070005 => codes::PERMISSION_DENIED,
        0x80070020 => codes::FILE_LOCKED,
        // WINCODEC_ERR_COMPONENTNOTFOUND: no codec (HEIF/HEVC extension missing)
        0x88982F50 => codes::ENGINE_MISSING,
        0x88982F61 | 0x88982F60 => codes::CORRUPT_FILE,
        _ => codes::ENGINE_FAILED,
    };
    FileError::with(code, json!({ "message": e.message().to_string(), "hresult": format!("0x{hr:08X}") }))
}

fn open_stream(path: &Path) -> Result<IRandomAccessStream, FileError> {
    open_mode(path, FileAccessMode::Read)
}

fn open_mode(path: &Path, mode: FileAccessMode) -> Result<IRandomAccessStream, FileError> {
    init();
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    unsafe {
        windows::Win32::System::WinRT::CreateRandomAccessStreamOnFile(&HSTRING::from(abs.as_os_str()), mode.0 as u32)
            .map_err(win_err)
    }
}

/// (width, height, frame count), orientation applied.
pub fn image_info(path: &Path) -> Result<(u32, u32, u32), FileError> {
    let stream = open_stream(path)?;
    let dec = BitmapDecoder::CreateAsync(&stream).and_then(|o| o.join()).map_err(win_err)?;
    let n = dec.FrameCount().map_err(win_err)?;
    Ok((dec.OrientedPixelWidth().map_err(win_err)?, dec.OrientedPixelHeight().map_err(win_err)?, n))
}

/// The resolution an image says it has (scanners write it; 96 when it says nothing).
pub fn image_dpi(path: &Path) -> Option<f64> {
    let stream = open_stream(path).ok()?;
    let dec = BitmapDecoder::CreateAsync(&stream).and_then(|o| o.join()).ok()?;
    dec.DpiX().ok()
}

/// Decodes frame `index` to RGBA8 (sRGB, straight alpha, orientation applied).
pub fn decode_image(path: &Path, index: u32) -> Result<(u32, u32, Vec<u8>), FileError> {
    let stream = open_stream(path)?;
    let dec = BitmapDecoder::CreateAsync(&stream).and_then(|o| o.join()).map_err(win_err)?;
    let frame = dec.GetFrameAsync(index).and_then(|o| o.join()).map_err(win_err)?;
    let w = frame.OrientedPixelWidth().map_err(win_err)?;
    let h = frame.OrientedPixelHeight().map_err(win_err)?;
    let t = BitmapTransform::new().map_err(win_err)?;
    let pd = frame
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Rgba8,
            BitmapAlphaMode::Straight,
            &t,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::ColorManageToSRgb,
        )
        .and_then(|o| o.join())
        .map_err(win_err)?;
    let data = pd.DetachPixelData().map_err(win_err)?;
    let v = data.to_vec();
    if v.len() as u64 != w as u64 * h as u64 * 4 {
        return Err(FileError::new(codes::CORRUPT_FILE));
    }
    Ok((w, h, v))
}

pub struct Pdf {
    doc: PdfDocument,
}

impl Pdf {
    pub fn open(path: &Path, password: Option<&str>) -> Result<Pdf, FileError> {
        let stream = open_stream(path)?;
        let res = match password {
            Some(p) => PdfDocument::LoadFromStreamWithPasswordAsync(&stream, &HSTRING::from(p)).and_then(|o| o.join()),
            None => PdfDocument::LoadFromStreamAsync(&stream).and_then(|o| o.join()),
        };
        let doc = res.map_err(|e| {
            // 0x8007052B = ERROR_WRONG_PASSWORD
            if e.code().0 as u32 == 0x8007052B {
                FileError::new(if password.is_some() { codes::WRONG_PASSWORD } else { codes::PASSWORD_REQUIRED })
            } else {
                win_err(e)
            }
        })?;
        Ok(Pdf { doc })
    }

    pub fn page_count(&self) -> u32 {
        self.doc.PageCount().unwrap_or(0)
    }

    /// Size of page `index` (0-based) as displayed, in device-independent pixels (1/96 in).
    pub fn page_size(&self, index: u32) -> Result<(f64, f64), FileError> {
        let size = self.doc.GetPage(index).and_then(|p| p.Size()).map_err(win_err)?;
        Ok((size.Width as f64, size.Height as f64))
    }

    /// Renders page `index` (0-based) at `dpi` to PNG bytes.
    pub fn render_png(&self, index: u32, dpi: f64, transparent: bool) -> Result<Vec<u8>, FileError> {
        let page = self.doc.GetPage(index).map_err(win_err)?;
        let size = page.Size().map_err(win_err)?; // device-independent pixels (1/96 in)
        let w = ((size.Width as f64) * dpi / 96.0).round().max(1.0) as u32;
        let h = ((size.Height as f64) * dpi / 96.0).round().max(1.0) as u32;
        let png = render_page(&page, w, h, transparent)?;
        // Windows may scale the bitmap by the display's scaling (seen: 1.4× on a 150%
        // screen). Ask again, corrected, so the size is what the user chose.
        match png_size(&png) {
            Some((aw, ah)) if aw != w || ah != h => {
                let (kx, ky) = (aw as f64 / w as f64, ah as f64 / h as f64);
                let png = render_page(&page, ((w as f64 / kx).round() as u32).max(1), ((h as f64 / ky).round() as u32).max(1), transparent)?;
                Ok(png)
            }
            _ => Ok(png),
        }
    }
}

fn render_page(page: &windows::Data::Pdf::PdfPage, w: u32, h: u32, transparent: bool) -> Result<Vec<u8>, FileError> {
    let opts = PdfPageRenderOptions::new().map_err(win_err)?;
    opts.SetDestinationWidth(w).map_err(win_err)?;
    opts.SetDestinationHeight(h).map_err(win_err)?;
    opts.SetBitmapEncoderId(BitmapEncoder::PngEncoderId().map_err(win_err)?).map_err(win_err)?;
    if transparent {
        opts.SetBackgroundColor(windows::UI::Color { A: 0, R: 255, G: 255, B: 255 }).map_err(win_err)?;
    }
    let stream = InMemoryRandomAccessStream::new().map_err(win_err)?;
    page.RenderWithOptionsToStreamAsync(&stream, &opts).and_then(|o| o.join()).map_err(win_err)?;
    let len = stream.Size().map_err(win_err)? as u32;
    let input = stream.GetInputStreamAt(0).map_err(win_err)?;
    let reader = DataReader::CreateDataReader(&input).map_err(win_err)?;
    reader.LoadAsync(len).and_then(|o| o.join()).map_err(win_err)?;
    let mut buf = vec![0u8; len as usize];
    reader.ReadBytes(&mut buf).map_err(win_err)?;
    Ok(buf)
}

/// Width and height from a PNG's header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let w = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

/// Development only: re-encodes an image as HEIC with the Windows HEIF encoder,
/// with an EXIF orientation, to produce test files.
#[cfg(debug_assertions)]
pub fn dev_encode_heic(input: &Path, output: &Path, orientation: u16) -> Result<(), FileError> {
    use windows::Graphics::Imaging::{BitmapPropertySet, BitmapTypedValue};
    use windows::Foundation::PropertyValue;
    let (w, h, rgba) = decode_image(input, 0)?;
    std::fs::write(output, b"").map_err(crate::plugins::io_err)?;
    let out = open_rw(output)?;
    let enc = BitmapEncoder::CreateAsync(BitmapEncoder::HeifEncoderId().map_err(win_err)?, &out).and_then(|o| o.join()).map_err(win_err)?;
    enc.SetPixelData(BitmapPixelFormat::Rgba8, BitmapAlphaMode::Straight, w, h, 96.0, 96.0, &rgba).map_err(win_err)?;
    if orientation != 1 {
        let props = BitmapPropertySet::new().map_err(win_err)?;
        let v = PropertyValue::CreateUInt16(orientation).map_err(win_err)?;
        let tv = BitmapTypedValue::Create(&v, windows::Foundation::PropertyType::UInt16).map_err(win_err)?;
        props.Insert(&HSTRING::from("System.Photo.Orientation"), &tv).map_err(win_err)?;
        enc.BitmapProperties().map_err(win_err)?.SetPropertiesAsync(&props).and_then(|o| o.join()).map_err(win_err)?;
    }
    enc.FlushAsync().and_then(|o| o.join()).map_err(win_err)?;
    Ok(())
}

#[cfg(debug_assertions)]
fn open_rw(path: &Path) -> Result<IRandomAccessStream, FileError> {
    open_mode(path, FileAccessMode::ReadWrite)
}

/// Installed Windows voices: (id, name, language), as Settings › Speech lists them.
pub fn voices() -> Result<Vec<(String, String, String)>, FileError> {
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    init();
    let all = SpeechSynthesizer::AllVoices().map_err(win_err)?;
    let mut out = Vec::new();
    for i in 0..all.Size().map_err(win_err)? {
        let v = all.GetAt(i).map_err(win_err)?;
        out.push((v.Id().map_err(win_err)?.to_string(), v.DisplayName().map_err(win_err)?.to_string(), v.Language().map_err(win_err)?.to_string()));
    }
    Ok(out)
}

/// Reads `text` aloud with a Windows voice into a WAV file in memory.
pub fn speak(text: &str, voice_id: Option<&str>, rate: f64) -> Result<Vec<u8>, FileError> {
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    init();
    let synth = SpeechSynthesizer::new().map_err(win_err)?;
    if let Some(id) = voice_id {
        let all = SpeechSynthesizer::AllVoices().map_err(win_err)?;
        for i in 0..all.Size().map_err(win_err)? {
            let v = all.GetAt(i).map_err(win_err)?;
            if v.Id().map_err(win_err)? == id {
                synth.SetVoice(&v).map_err(win_err)?;
                break;
            }
        }
    }
    synth.Options().and_then(|o| o.SetSpeakingRate(rate.clamp(0.5, 3.0))).map_err(win_err)?;
    let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(text)).and_then(|o| o.join()).map_err(win_err)?;
    let len = stream.Size().map_err(win_err)? as u32;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(win_err)?).map_err(win_err)?;
    reader.LoadAsync(len).and_then(|o| o.join()).map_err(win_err)?;
    let mut buf = vec![0u8; len as usize];
    reader.ReadBytes(&mut buf).map_err(win_err)?;
    Ok(buf)
}

/// The user's first Windows display language, as a primary subtag ("de", "pl", "ro"…).
pub fn user_language() -> String {
    init();
    windows::Globalization::ApplicationLanguages::Languages()
        .ok()
        .and_then(|l| l.GetAt(0).ok())
        .map(|t| t.to_string().split('-').next().unwrap_or("en").to_ascii_lowercase())
        .unwrap_or_else(|| "en".into())
}

/// One recognized word: its text and box in pixels (origin top-left).
pub struct OcrWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The languages Windows can read (Settings › Time & language › Language packs).
pub fn ocr_languages() -> Result<Vec<(String, String)>, FileError> {
    use windows::Media::Ocr::OcrEngine;
    init();
    let all = OcrEngine::AvailableRecognizerLanguages().map_err(win_err)?;
    let mut out = Vec::new();
    for i in 0..all.Size().map_err(win_err)? {
        let l = all.GetAt(i).map_err(win_err)?;
        out.push((l.LanguageTag().map_err(win_err)?.to_string(), l.DisplayName().map_err(win_err)?.to_string()));
    }
    Ok(out)
}

pub fn ocr_max_dimension() -> u32 {
    windows::Media::Ocr::OcrEngine::MaxImageDimension().unwrap_or(10000)
}

/// Lines of words found in an encoded image (PNG, JPEG…).
pub fn ocr_image(data: &[u8], lang: Option<&str>) -> Result<Vec<Vec<OcrWord>>, FileError> {
    use windows::Globalization::Language;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    init();
    let engine = match lang.filter(|l| !l.is_empty()) {
        Some(l) => OcrEngine::TryCreateFromLanguage(&Language::CreateLanguage(&HSTRING::from(l)).map_err(win_err)?),
        None => OcrEngine::TryCreateFromUserProfileLanguages(),
    }
    .map_err(win_err)?;
    let mem = InMemoryRandomAccessStream::new().map_err(win_err)?;
    let writer = DataWriter::CreateDataWriter(&mem).map_err(win_err)?;
    writer.WriteBytes(data).map_err(win_err)?;
    writer.StoreAsync().and_then(|o| o.join()).map_err(win_err)?;
    writer.DetachStream().map_err(win_err)?;
    mem.Seek(0).map_err(win_err)?;
    let dec = BitmapDecoder::CreateAsync(&mem).and_then(|o| o.join()).map_err(win_err)?;
    let bmp = dec.GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied).and_then(|o| o.join()).map_err(win_err)?;
    let result = engine.RecognizeAsync(&bmp).and_then(|o| o.join()).map_err(win_err)?;
    let mut lines = Vec::new();
    let ls = result.Lines().map_err(win_err)?;
    for i in 0..ls.Size().map_err(win_err)? {
        let line = ls.GetAt(i).map_err(win_err)?;
        let ws = line.Words().map_err(win_err)?;
        let mut words = Vec::new();
        for j in 0..ws.Size().map_err(win_err)? {
            let w = ws.GetAt(j).map_err(win_err)?;
            let r = w.BoundingRect().map_err(win_err)?;
            words.push(OcrWord { text: w.Text().map_err(win_err)?.to_string(), x: r.X as f64, y: r.Y as f64, w: r.Width as f64, h: r.Height as f64 });
        }
        lines.push(words);
    }
    Ok(lines)
}
