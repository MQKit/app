mod ai_upscale;
mod archives;
mod audio_convert;
mod cad_3d;
mod avif;
mod fonts;
mod pdf_to_excel;
mod raw;
mod remove_background;
mod design_files;
mod ebooks;
mod heic_to_jpg;
mod html_to_pdf;
mod icons;
mod mail;
mod markdown;
mod media_merge;
mod media_trim;
mod meta_strip;
mod ocr;
mod office_to_pdf;
mod image_compress;
mod image_convert;
mod image_resize;
mod images_to_pdf;
mod pdf_compress;
mod pdf_merge;
mod pdf_pages;
mod pdf_password;
mod pdf_split;
mod pdf_stamp;
mod pdf_to_images;
mod pdf_to_pptx;
mod pdf_to_word_exact;
mod svg;
mod subtitles;
mod tabular;
mod transcribe;
mod tts;
mod video_compress;
mod video_convert;
mod video_gif;
mod video_to_mp3;
mod word_to_pdf_lo;

use contract::{codes, Cancel, Emitter, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

pub fn all() -> Vec<&'static dyn Plugin> {
    vec![
        &pdf_compress::PdfCompress,
        &pdf_merge::PdfMerge,
        &pdf_split::PdfSplit,
        &pdf_password::PdfPassword,
        &pdf_pages::PdfPages,
        &images_to_pdf::ImagesToPdf,
        &pdf_to_images::PdfToImages,
        &image_compress::ImageCompress,
        &image_convert::ImageConvert,
        &image_resize::ImageResize,
        &icons::Icons,
        &heic_to_jpg::HeicToJpg,
        &video_to_mp3::VideoToMp3,
        &video_compress::VideoCompress,
        &video_convert::VideoConvert,
        &audio_convert::AudioConvert,
        &media_trim::MediaTrim,
        &video_gif::VideoGif,
        &media_merge::MediaMerge,
        &meta_strip::MetaStrip,
        &subtitles::Subtitles,
        &html_to_pdf::HtmlToPdf,
        &markdown::Markdown,
        &office_to_pdf::OfficeToPdf,
        &tts::Tts,
        &ocr::Ocr,
        &pdf_stamp::PdfStamp,
        &tabular::Tabular,
        &avif::Avif,
        &pdf_to_pptx::PdfToPptx,
        &svg::Svg,
        &archives::Archives,
        &fonts::Fonts,
        &pdf_to_excel::PdfToExcel,
        &remove_background::RemoveBackground,
        &ai_upscale::AiUpscale,
        &transcribe::Transcribe,
        &mail::MailTool,
        &raw::Raw,
        &design_files::DesignFiles,
        &ebooks::Ebooks,
        &cad_3d::Cad3d,
        &word_to_pdf_lo::WordToPdfLo,
        &pdf_to_word_exact::PdfToWordExact,
    ]
}

pub fn find(id: &str) -> Option<&'static dyn Plugin> {
    all().into_iter().find(|p| p.id() == id)
}

pub struct Analysis {
    pub status: &'static str,
    pub props: Map<String, Value>,
    pub warnings: Vec<Value>,
    pub code: Option<String>,
}

impl Analysis {
    pub fn ok(props: Map<String, Value>) -> Self {
        Analysis { status: "ok", props, warnings: vec![], code: None }
    }
    pub fn failed(code: &str) -> Self {
        Analysis { status: "failed", props: Map::new(), warnings: vec![], code: Some(code.into()) }
    }
    pub fn status(status: &'static str, code: &str) -> Self {
        Analysis { status, props: Map::new(), warnings: vec![], code: Some(code.into()) }
    }
}

pub trait Plugin: Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str {
        "0.1.0"
    }
    fn manifest(&self) -> Value;
    /// An extra entry of another tool's manifest (no plugin.json of its own).
    fn hidden(&self) -> bool {
        false
    }
    fn probe(&self) -> Value {
        json!({})
    }
    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis;
    /// `preview` with `target.kind = estimate`: `{outputBytes, confidence, method}`.
    fn estimate(&self, _req: &Request) -> Option<Value> {
        None
    }
    /// `preview` with `target.kind = crop`: the `preview` message (before/after images).
    fn preview(&self, _req: &Request) -> Result<Option<Value>, FileError> {
        Ok(None)
    }
    /// Converts one file. Writes into `ctx.file_dir(f)` and emits `output`.
    fn run_file(&self, _ctx: &Ctx, _f: &FileRef) -> Result<Value, FileError> {
        Err(FileError::new(codes::UNSUPPORTED_FEATURE))
    }
    /// The whole job. The default runs `run_file` for each file.
    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        for f in &ctx.req.files {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || self.run_file(ctx, f));
        }
        Ok(())
    }
}

pub struct Ctx<'a> {
    pub req: &'a Request,
    pub out: Emitter,
    pub cancel: Cancel,
    pub out_dir: PathBuf,
    pub work_dir: PathBuf,
    counts: Mutex<(u32, u32, u32)>,
    last_progress: Mutex<Option<(Instant, String, f64)>>,
}

impl<'a> Ctx<'a> {
    pub fn new(req: &'a Request, out: Emitter, cancel: Cancel, out_dir: PathBuf, work_dir: PathBuf) -> Self {
        Ctx { req, out, cancel, out_dir, work_dir, counts: Mutex::new((0, 0, 0)), last_progress: Mutex::new(None) }
    }

    /// `outDir/<id>/`, created on demand.
    pub fn file_dir(&self, id: &str) -> Result<PathBuf, FileError> {
        let d = self.out_dir.join(sanitize(id));
        std::fs::create_dir_all(&d).map_err(|e| {
            // A full disk or a locked folder is said as such; a folder that can't be made
            // (a drive that went away, a path that is a file) is "can't write there".
            let mut err = io_err(e);
            if err.code == codes::UNSUPPORTED_FILE || err.code == codes::ENGINE_FAILED {
                err.code = codes::PERMISSION_DENIED.into();
            }
            err
        })?;
        Ok(d)
    }

    /// Progress, about 10 times per second at most. Always sent: the start (0), the
    /// end (1), a new stage, and jumps of a quarter or more, so even fast files show steps.
    pub fn progress(&self, file: &str, fraction: f64, stage_code: &str, stage: u32, stages: u32) {
        let mut last = self.last_progress.lock().unwrap();
        let now = Instant::now();
        let due = match last.as_ref() {
            None => true,
            Some((t, code, f)) => {
                now.duration_since(*t).as_millis() >= 100 || code != stage_code || (fraction - f).abs() >= 0.25
            }
        };
        if due || fraction <= 0.0 || fraction >= 1.0 {
            *last = Some((now, stage_code.to_string(), fraction));
            self.out.progress(file, fraction, stage_code, stage, stages, None);
        }
    }

    pub fn check_cancel(&self) -> Result<(), FileError> {
        if self.cancel.is_cancelled() {
            Err(FileError::new(codes::CANCELLED))
        } else {
            Ok(())
        }
    }

    /// A whole folder as one result (the app moves it into place).
    pub fn emit_folder(&self, file: &str, path: &Path, vars: Value, input_bytes: u64) {
        fn size(d: &Path) -> u64 {
            std::fs::read_dir(d)
                .map(|rd| rd.flatten().map(|e| if e.path().is_dir() { size(&e.path()) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }).sum())
                .unwrap_or(0)
        }
        self.out.output(file, path, "folder", vars, json!({"inputBytes": input_bytes, "outputBytes": size(path)}));
    }

    pub fn emit_output(&self, file: &str, path: &Path, ext: &str, vars: Value, input_bytes: u64) {
        let output_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        self.out.output(file, path, ext, vars, json!({"inputBytes": input_bytes, "outputBytes": output_bytes}));
    }

    /// Runs one unit of work with fileStart/fileDone and panic isolation.
    pub fn run_one(&self, file: &str, f: impl FnOnce() -> Result<Value, FileError>) {
        self.out.file_start(file);
        let res = catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            Err(FileError::with(codes::ENGINE_FAILED, json!({ "message": msg })))
        });
        let mut c = self.counts.lock().unwrap();
        match res {
            Ok(resolved) => {
                c.0 += 1;
                let mut m = json!({"type":"fileDone","file":file,"status":"ok"});
                if !resolved.is_null() {
                    m["resolved"] = resolved;
                }
                self.out.emit(m);
            }
            Err(e) => {
                if e.skipped {
                    c.1 += 1;
                } else {
                    c.2 += 1;
                }
                let mut m = json!({"type":"fileDone","file":file,"status": if e.skipped {"skipped"} else {"failed"},"code":e.code});
                if let Some(d) = e.details {
                    m["details"] = d;
                }
                if let Some(s) = e.suggest {
                    m["suggest"] = s;
                }
                self.out.emit(m);
            }
        }
    }

    pub fn summary(&self) -> Value {
        let c = self.counts.lock().unwrap();
        json!({"ok": c.0, "skipped": c.1, "failed": c.2})
    }
}

fn sanitize(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Shared helper for analysis: file size.
/// The file a preview is for: `req.file`, else the first one.
pub fn preview_file(req: &Request) -> Option<&FileRef> {
    req.files.iter().find(|f| Some(&f.id) == req.file.as_ref()).or(req.files.first())
}

pub fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Writes `data` to `path`, mapping disk errors to contract codes.
pub fn write_file(path: &Path, data: &[u8]) -> Result<(), FileError> {
    std::fs::write(path, data).map_err(io_err)
}

pub fn io_err(e: std::io::Error) -> FileError {
    let code = match e.kind() {
        std::io::ErrorKind::PermissionDenied => codes::PERMISSION_DENIED,
        std::io::ErrorKind::StorageFull => codes::DISK_FULL,
        std::io::ErrorKind::NotFound => codes::UNSUPPORTED_FILE,
        _ if e.raw_os_error() == Some(32) || e.raw_os_error() == Some(33) => codes::FILE_LOCKED,
        _ => codes::ENGINE_FAILED,
    };
    FileError::with(code, json!({ "message": e.to_string() }))
}
