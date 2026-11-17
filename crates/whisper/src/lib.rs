//! Speech to text with whisper.cpp (compiled in, CPU). A small safe wrapper over the C
//! shim in shim.cpp.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;

#[repr(C)]
struct Ctx(c_void);

type ProgressFn = extern "C" fn(c_int, *mut c_void);
type AbortFn = extern "C" fn(*mut c_void) -> bool;

extern "C" {
    fn shim_quiet();
    fn shim_load(model: *const c_char) -> *mut Ctx;
    fn shim_free(ctx: *mut Ctx);
    #[allow(clippy::too_many_arguments)]
    fn shim_run(
        ctx: *mut Ctx, samples: *const f32, n: c_int, language: *const c_char, translate: bool, threads: c_int, max_len: c_int,
        beam_size: c_int, prompt: *const c_char, progress: ProgressFn, abort: AbortFn, user: *mut c_void,
    ) -> c_int;
    fn shim_n_segments(ctx: *mut Ctx) -> c_int;
    fn shim_segment_text(ctx: *mut Ctx, i: c_int) -> *const c_char;
    fn shim_segment_t0(ctx: *mut Ctx, i: c_int) -> i64;
    fn shim_segment_t1(ctx: *mut Ctx, i: c_int) -> i64;
    fn shim_language(ctx: *mut Ctx) -> *const c_char;
}

/// A loaded model.
pub struct Whisper {
    ctx: *mut Ctx,
}

// The context is used from one thread at a time.
unsafe impl Send for Whisper {}

/// What to do.
pub struct Options<'a> {
    /// A language code ("de", "ro", "ja"…) or "auto".
    pub language: &'a str,
    pub translate: bool,
    pub threads: usize,
    /// Characters per segment (short subtitle lines, cut between words); 0 = whole sentences.
    pub max_len: usize,
    pub beam_size: usize,
    /// Names and rare words, to help the spelling.
    pub prompt: &'a str,
}

/// One piece of speech, times in hundredths of a second.
pub struct Segment {
    pub start: i64,
    pub end: i64,
    pub text: String,
}

pub enum Error {
    Stopped,
    Failed,
}

struct Callbacks<'a> {
    progress: &'a mut dyn FnMut(u32),
    stop: &'a dyn Fn() -> bool,
}

extern "C" fn on_progress(p: c_int, user: *mut c_void) {
    let cb = unsafe { &mut *(user as *mut Callbacks) };
    (cb.progress)(p.clamp(0, 100) as u32);
}

extern "C" fn on_abort(user: *mut c_void) -> bool {
    let cb = unsafe { &*(user as *const Callbacks) };
    (cb.stop)()
}

impl Whisper {
    pub fn load(model: &Path) -> Option<Whisper> {
        let path = CString::new(model.to_string_lossy().as_bytes()).ok()?;
        unsafe { shim_quiet() };
        let ctx = unsafe { shim_load(path.as_ptr()) };
        (!ctx.is_null()).then_some(Whisper { ctx })
    }

    /// Transcribes 16 kHz mono samples. `progress` gets 0…100; `stop` is asked now and then.
    pub fn run(&mut self, samples: &[f32], o: &Options, progress: &mut dyn FnMut(u32), stop: &dyn Fn() -> bool) -> Result<(Vec<Segment>, String), Error> {
        let language = CString::new(o.language).map_err(|_| Error::Failed)?;
        let prompt = CString::new(o.prompt).map_err(|_| Error::Failed)?;
        let mut cb = Callbacks { progress, stop };
        let r = unsafe {
            shim_run(
                self.ctx, samples.as_ptr(), samples.len() as c_int, language.as_ptr(), o.translate, o.threads.max(1) as c_int,
                o.max_len as c_int, o.beam_size as c_int, prompt.as_ptr(), on_progress, on_abort, &mut cb as *mut Callbacks as *mut c_void,
            )
        };
        match r {
            0 => {}
            1 => return Err(Error::Stopped),
            _ => return Err(Error::Failed),
        }
        let mut out = Vec::new();
        for i in 0..unsafe { shim_n_segments(self.ctx) } {
            let text = unsafe { CStr::from_ptr(shim_segment_text(self.ctx, i)) }.to_string_lossy().trim().to_string();
            if !text.is_empty() {
                out.push(Segment { start: unsafe { shim_segment_t0(self.ctx, i) }, end: unsafe { shim_segment_t1(self.ctx, i) }, text });
            }
        }
        let lang = unsafe { shim_language(self.ctx) };
        let lang = if lang.is_null() { String::new() } else { unsafe { CStr::from_ptr(lang) }.to_string_lossy().into_owned() };
        Ok((out, lang))
    }
}

impl Drop for Whisper {
    fn drop(&mut self) {
        unsafe { shim_free(self.ctx) };
    }
}
