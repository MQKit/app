//! Vocals and accompaniment apart, with Deezer's Spleeter (2 stems, MIT) as ONNX: the
//! song's spectrogram goes through both models, their outputs become soft masks, and
//! the masked spectrogram is turned back into sound. Runs on the graphics card or the
//! processor; nothing is uploaded.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::{audio, ffmpeg, onnx};
use contract::{codes, opt, t, FileError, FileRef, Request};
use ort::session::Session;
use ort::value::Tensor;
use rustfft::{num_complex::Complex32, FftPlanner};
use serde_json::{json, Value};

pub struct MusicSplit;

const RATE: u32 = 44100;
const N: usize = 4096;
const HOP: usize = 1024;
/// Bins the models see (up to 11 kHz), and frames per segment.
const F: usize = 1024;
const T: usize = 512;
const ENGINE: &str = "spleeter";

const INPUTS: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "aiff", "aif", "alac", "ape", "wv", "mp4", "mkv", "webm", "mov"];

impl Plugin for MusicSplit {
    fn id(&self) -> &'static str {
        "music-split"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Voce și instrumental", "Vocals and instrumental"),
            "summary": t("Separă vocea de muzică, cu AI", "Split the voice from the music, with AI"),
            "keywords": { "ro": ["karaoke", "elimină vocea", "instrumental", "acapella", "separare"], "en": ["karaoke", "vocal remover", "instrumental", "acapella", "stem splitter"] },
            "category": "audio",
            "icon": "microphone-2",
            "requires": [ { "engine": "ffmpeg" }, { "engine": ENGINE } ],
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "multiPerInput", "extensions": ["mp3", "wav", "flac"],
                        "nameVariables": [ { "id": "part", "label": t("partea", "part") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["durationSec", "hasAudio"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 3.0 },
            "presets": [
                { "id": "both", "label": t("Voce și instrumental", "Vocals and instrumental"), "default": true, "values": { "keep": "both" } },
                { "id": "karaoke", "label": t("Karaoke (fără voce)", "Karaoke (no vocals)"), "values": { "keep": "instrumental" } },
                { "id": "acapella", "label": t("Doar vocea", "Vocals only"), "values": { "keep": "vocals" } }
            ],
            "fields": [
                { "key": "keep", "type": "select", "label": t("Ce păstrezi", "What to keep"), "default": "both", "display": "chips",
                  "options": [ opt("both", "Amândouă", "Both"), opt("instrumental", "Instrumentalul", "The instrumental"), opt("vocals", "Vocea", "The vocals") ] },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "mp3", "display": "chips",
                  "options": [ opt("mp3", "MP3", "MP3"), opt("wav", "WAV", "WAV"), opt("flac", "FLAC", "FLAC") ] }
            ],
            "rules": [
                { "id": "no_audio", "when": { "file": "hasAudio", "eq": false }, "severity": "warning",
                  "message": t("Unele fișiere nu au sunet și vor fi sărite.", "Some files have no sound and will be skipped.") },
                { "id": "long", "when": { "file": "durationSec", "gt": 900, "quant": "any" }, "severity": "info",
                  "message": t("Fișierele lungi au nevoie de câteva minute și de multă memorie.", "Long files need a few minutes and a lot of memory.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_{part}", "{name}_{part}"), "outputDir": "subfolder", "subfolderName": "{name}" } },
            "errors": { "no_audio": t("Fișierul nu are sunet.", "The file has no sound.") }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        match ffmpeg::probe(req, f.input_path()) {
            Ok(p) => Analysis::ok(p),
            Err(e) if e.code == codes::UNSUPPORTED_FILE => Analysis::status("unsupported", codes::UNSUPPORTED_FILE),
            Err(e) if e.code == codes::CORRUPT_FILE => Analysis::status("corrupt", codes::CORRUPT_FILE),
            Err(e) => Analysis::status("failed", &e.code),
        }
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        // The two models load once for all the files.
        let mut models: Option<(Session, Session)> = None;
        for f in &ctx.req.files {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || split_file(ctx, f, &mut models));
        }
        Ok(())
    }
}

fn split_file(ctx: &Ctx, f: &FileRef, models: &mut Option<(Session, Session)>) -> Result<Value, FileError> {
    let req = ctx.req;
    let input = f.input_path();
    let in_bytes = file_size(input);
    if f.analysis.get("hasAudio").and_then(Value::as_bool) == Some(false) {
        return Err(FileError::new("no_audio"));
    }
    ctx.progress(&f.id, 0.0, "read", 1, 3);
    let pcm = audio::decode(req, input, RATE, 2, 0.0)?;
    let len = pcm.len() / 2;
    if len < N {
        return Err(FileError::new("no_audio"));
    }
    let chans: [Vec<f32>; 2] = [pcm.iter().step_by(2).copied().collect(), pcm.iter().skip(1).step_by(2).copied().collect()];
    drop(pcm);
    ctx.check_cancel()?;
    if models.is_none() {
        ctx.progress(&f.id, 0.05, "model", 2, 3);
        let threads = req.limits.threads.max(1) as usize;
        let (v, _) = onnx::session(req, &onnx::model(req, ENGINE, "vocals.fp16.onnx")?, threads)?;
        let (a, _) = onnx::session(req, &onnx::model(req, ENGINE, "accompaniment.fp16.onnx")?, threads)?;
        *models = Some((v, a));
    }
    let (vm, am) = models.as_mut().unwrap();

    // The spectrogram of each channel (after a frame of silence, as Spleeter does).
    let specs: Vec<Vec<Vec<Complex32>>> = chans.iter().map(|c| stft(c)).collect();
    let frames = specs[0].len();
    let segments = frames.div_ceil(T);
    let mut masks_v = vec![vec![0f32; F]; frames * 2];
    // A few segments per run of the models: progress shows, memory stays small.
    let per_run = 4;
    let mut seg = 0;
    while seg < segments {
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.1 + 0.75 * seg as f64 / segments as f64, "separate", 2, 3);
        let n = per_run.min(segments - seg);
        // [channel, segment, frame, bin], magnitudes; frames past the end are zero.
        let mut x = vec![0f32; 2 * n * T * F];
        for (ch, spec) in specs.iter().enumerate() {
            for s in 0..n {
                for t in 0..T {
                    let fr = (seg + s) * T + t;
                    if fr >= frames {
                        break;
                    }
                    let base = ((ch * n + s) * T + t) * F;
                    for b in 0..F {
                        x[base + b] = spec[fr][b].norm();
                    }
                }
            }
        }
        let shape = [2usize, n, T, F];
        let run = |m: &mut Session, data: Vec<f32>| -> Result<Vec<f32>, FileError> {
            let input = Tensor::from_array((shape, data.into_boxed_slice())).map_err(FileError::engine)?;
            let name = m.inputs()[0].name().to_string();
            let out = m.run(ort::inputs![name => input]).map_err(FileError::engine)?;
            let (_, v) = out[0].try_extract_tensor::<f32>().map_err(FileError::engine)?;
            Ok(v.to_vec())
        };
        let v = run(vm, x.clone())?;
        let a = run(am, x)?;
        if v.len() < 2 * n * T * F || a.len() < 2 * n * T * F {
            return Err(FileError::engine("unexpected model output"));
        }
        for ch in 0..2 {
            for s in 0..n {
                for t in 0..T {
                    let fr = (seg + s) * T + t;
                    if fr >= frames {
                        break;
                    }
                    let base = ((ch * n + s) * T + t) * F;
                    let row = &mut masks_v[ch * frames + fr];
                    for b in 0..F {
                        let (vv, aa) = (v[base + b] * v[base + b], a[base + b] * a[base + b]);
                        // Soft masks (power ratio); the two add up to 1.
                        row[b] = (vv + 5e-11) / (vv + aa + 1e-10);
                    }
                }
            }
        }
        seg += n;
    }
    ctx.progress(&f.id, 0.85, "write", 3, 3);

    let keep = req.str_or("keep", "both");
    let format = match req.str_or("format", "mp3") {
        "wav" => "wav",
        "flac" => "flac",
        _ => "mp3",
    };
    let dir = ctx.file_dir(&f.id)?;
    let parts: Vec<(&str, bool)> = match keep {
        "vocals" => vec![("vocals", true)],
        "instrumental" => vec![("instrumental", false)],
        _ => vec![("vocals", true), ("instrumental", false)],
    };
    let words = crate::common::words::of(req.locale.as_deref());
    for (i, (part, vocal)) in parts.iter().enumerate() {
        ctx.check_cancel()?;
        let mut out = vec![0f32; len * 2];
        for ch in 0..2 {
            let masked: Vec<Vec<Complex32>> = specs[ch].iter().enumerate().map(|(fr, frame)| {
                let m = &masks_v[ch * frames + fr];
                // Above 11 kHz the models say nothing: the frame's average mask goes on.
                let avg = m.iter().sum::<f32>() / F as f32;
                frame.iter().enumerate().map(|(b, c)| {
                    let mv = if b < F { m[b] } else { avg };
                    c * if *vocal { mv } else { 1.0 - mv }
                }).collect()
            }).collect();
            let wave = istft(&masked, len);
            for (k, s) in wave.into_iter().enumerate() {
                out[k * 2 + ch] = s;
            }
        }
        let path = dir.join(format!("{part}.{format}"));
        audio::encode(req, &out, RATE, 2, &path)?;
        let label = if *vocal { words.vocals } else { words.instrumental };
        ctx.emit_output(&f.id, &path, format, json!({ "part": label }), if i == 0 { in_bytes } else { 0 });
    }
    ctx.progress(&f.id, 1.0, "write", 3, 3);
    Ok(Value::Null)
}

/// Spleeter's STFT: a frame of zeros first, Hann frames of N every HOP, padded at the end.
fn stft(x: &[f32]) -> Vec<Vec<Complex32>> {
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let w = audio::hann(N);
    let total = N + x.len();
    let frames = total.div_ceil(HOP);
    let at = |i: usize| if i >= N && i - N < x.len() { x[i - N] } else { 0.0 };
    let mut out = Vec::with_capacity(frames);
    let mut buf = vec![Complex32::new(0.0, 0.0); N];
    for k in 0..frames {
        let start = k * HOP;
        for i in 0..N {
            buf[i] = Complex32::new(at(start + i) * w[i], 0.0);
        }
        fft.process(&mut buf);
        out.push(buf[..N / 2 + 1].to_vec());
    }
    out
}

/// The inverse: overlap-added frames, divided by the summed squared window; the leading
/// frame of zeros taken off, `len` samples kept.
fn istft(spec: &[Vec<Complex32>], len: usize) -> Vec<f32> {
    let ifft = FftPlanner::<f32>::new().plan_fft_inverse(N);
    let w = audio::hann(N);
    let total = spec.len() * HOP + N;
    let mut acc = vec![0f32; total];
    let mut norm = vec![0f32; total];
    let mut buf = vec![Complex32::new(0.0, 0.0); N];
    for (k, frame) in spec.iter().enumerate() {
        // The full spectrum from its first half (a real signal's is symmetric).
        buf[..=N / 2].copy_from_slice(&frame[..=N / 2]);
        for b in N / 2 + 1..N {
            buf[b] = frame[N - b].conj();
        }
        ifft.process(&mut buf);
        let start = k * HOP;
        for i in 0..N {
            acc[start + i] += buf[i].re / N as f32 * w[i];
            norm[start + i] += w[i] * w[i];
        }
    }
    (0..len).map(|i| {
        let j = i + N;
        if j < total && norm[j] > 1e-6 { acc[j] / norm[j] } else { 0.0 }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stft_round_trip() {
        // A mask of all ones gives the sound back.
        let x: Vec<f32> = (0..50_000).map(|i| ((i as f32) * 0.01).sin() * 0.5 + ((i as f32) * 0.37).sin() * 0.2).collect();
        let y = istft(&stft(&x), x.len());
        let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(err < 1e-3, "{err}");
    }
}
