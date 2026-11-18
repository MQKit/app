//! Audio as numbers, through FFmpeg: any file decoded to 32-bit float samples, and
//! samples written back to a file. Tempo and key detection too (onset autocorrelation;
//! chroma against the Krumhansl–Kessler key profiles).

use super::ffmpeg;
use contract::{codes, FileError, Request};
use rustfft::{num_complex::Complex32, FftPlanner};
use serde_json::json;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;

/// Interleaved samples at `rate`, `channels` channels; at most `max_sec` seconds (0 = all).
pub fn decode(req: &Request, path: &Path, rate: u32, channels: u32, max_sec: f64) -> Result<Vec<f32>, FileError> {
    let mut cmd = ffmpeg::command(&ffmpeg::exe(req, "ffmpeg")?);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]).arg(path);
    if max_sec > 0.0 {
        cmd.args(["-t", &format!("{max_sec:.3}")]);
    }
    cmd.args(["-map", "0:a:0", "-vn", "-ac", &channels.to_string(), "-ar", &rate.to_string(), "-f", "f32le", "-"]);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "ffmpeg", "message": e.to_string() })))?;
    let mut raw = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut raw).map_err(crate::plugins::io_err)?;
    let mut err = String::new();
    let _ = child.stderr.take().unwrap().read_to_string(&mut err);
    let status = child.wait().map_err(crate::plugins::io_err)?;
    if !status.success() && raw.is_empty() {
        return Err(FileError::with(codes::CORRUPT_FILE, json!({ "message": err.trim() })));
    }
    Ok(raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// Writes interleaved samples to `out`, encoded as its extension says (wav, mp3, flac, m4a).
pub fn encode(req: &Request, samples: &[f32], rate: u32, channels: u32, out: &Path) -> Result<(), FileError> {
    let ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let codec: &[&str] = match ext.as_str() {
        "mp3" => &["-c:a", "libmp3lame", "-q:a", "1"],
        "flac" => &["-c:a", "flac"],
        "m4a" => &["-c:a", "aac", "-b:a", "256k"],
        _ => &["-c:a", "pcm_s16le"],
    };
    let mut child = ffmpeg::command(&ffmpeg::exe(req, "ffmpeg")?)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "f32le", "-ar", &rate.to_string(), "-ac", &channels.to_string(), "-i", "-"])
        .args(codec)
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "ffmpeg", "message": e.to_string() })))?;
    {
        let mut stdin = child.stdin.take().unwrap();
        let mut buf = Vec::with_capacity(1 << 20);
        for chunk in samples.chunks(1 << 18) {
            buf.clear();
            for s in chunk {
                buf.extend_from_slice(&s.clamp(-1.0, 1.0).to_le_bytes());
            }
            stdin.write_all(&buf).map_err(crate::plugins::io_err)?;
        }
    }
    let mut err = String::new();
    let _ = child.stderr.take().unwrap().read_to_string(&mut err);
    let status = child.wait().map_err(crate::plugins::io_err)?;
    if !status.success() {
        return Err(FileError::engine(err.trim().to_string()));
    }
    Ok(())
}

/// The periodic Hann window (as TensorFlow's and librosa's STFT use).
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect()
}

/// Magnitudes of the frames of `x` (mono): `n`-point windowed FFTs every `hop` samples,
/// `n / 2 + 1` bins each.
pub fn spectrogram(x: &[f32], n: usize, hop: usize) -> Vec<Vec<f32>> {
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let w = hann(n);
    let mut out = Vec::new();
    let mut buf = vec![Complex32::new(0.0, 0.0); n];
    let mut start = 0;
    while start + n <= x.len() {
        for i in 0..n {
            buf[i] = Complex32::new(x[start + i] * w[i], 0.0);
        }
        fft.process(&mut buf);
        out.push(buf[..n / 2 + 1].iter().map(|c| c.norm()).collect());
        start += hop;
    }
    out
}

/// Beats per minute of mono audio at `rate` Hz, or None if there is no steady beat.
pub fn tempo(x: &[f32], rate: u32) -> Option<f64> {
    let (n, hop) = (1024, 256);
    let spec = spectrogram(x, n, hop);
    if spec.len() < 200 {
        return None;
    }
    // Onset strength: how much louder each band got since the last frame (log scale).
    let logs: Vec<Vec<f32>> = spec.iter().map(|f| f.iter().map(|m| (1.0 + 100.0 * m).ln()).collect()).collect();
    let mut env: Vec<f32> = logs.windows(2).map(|w| w[1].iter().zip(&w[0]).map(|(a, b)| (a - b).max(0.0)).sum()).collect();
    // Minus the local average, so steady loud parts don't count as beats.
    let k = 16;
    let avg: Vec<f32> = (0..env.len()).map(|i| {
        let (a, b) = (i.saturating_sub(k), (i + k + 1).min(env.len()));
        env[a..b].iter().sum::<f32>() / (b - a) as f32
    }).collect();
    for (e, a) in env.iter_mut().zip(&avg) {
        *e = (*e - a).max(0.0);
    }
    let fps = rate as f64 / hop as f64;
    // Autocorrelation over the lags of 50–220 BPM, weighted toward the usual tempos.
    let lag = |bpm: f64| 60.0 * fps / bpm;
    let (lo, hi) = (lag(220.0).floor() as usize, lag(50.0).ceil() as usize);
    let ac = |l: usize| env.iter().zip(&env[l..]).map(|(a, b)| (a * b) as f64).sum::<f64>() / (env.len() - l) as f64;
    let scores: Vec<(usize, f64)> = (lo..=hi.min(env.len() / 2)).map(|l| {
        let bpm = 60.0 * fps / l as f64;
        let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        (l, ac(l) * prior)
    }).collect();
    let best = scores.iter().enumerate().max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))?;
    let (i, (l, s)) = (best.0, *best.1);
    if s <= 0.0 {
        return None;
    }
    // Between the neighbouring lags, for a tempo finer than whole frames.
    let at = |j: usize| scores.get(j).map(|v| v.1).unwrap_or(s);
    let (a, b, c) = (if i > 0 { at(i - 1) } else { s }, s, at(i + 1));
    let d = a - 2.0 * b + c;
    let off = if d.abs() > 1e-12 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 };
    let mut period = l as f64 + off;
    // Finer still: the peaks at two and four beats lie further out, where a frame is a
    // smaller share of the lag; each is found near its expected place and divided back.
    let mut periods = vec![period];
    for k in [2usize, 4] {
        let centre = (period * k as f64).round() as usize;
        let (a0, b0) = (centre.saturating_sub(3), centre + 3);
        if b0 + 1 >= env.len() / 2 {
            break;
        }
        let vals: Vec<(usize, f64)> = (a0..=b0).map(|j| (j, ac(j))).collect();
        let Some(m) = vals.iter().enumerate().max_by(|a, b| a.1 .1.total_cmp(&b.1 .1)).map(|(i, _)| i) else { break };
        if m == 0 || m == vals.len() - 1 {
            continue;
        }
        let (a, b, c) = (vals[m - 1].1, vals[m].1, vals[m + 1].1);
        let d = a - 2.0 * b + c;
        let off = if d.abs() > 1e-12 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 };
        periods.push((vals[m].0 as f64 + off) / k as f64);
    }
    period = periods.last().copied().unwrap_or(period);
    let mut bpm = 60.0 * fps / period;
    // Said the way musicians count it: between 70 and 180.
    while bpm < 70.0 {
        bpm *= 2.0;
    }
    while bpm > 180.0 {
        bpm /= 2.0;
    }
    Some((bpm * 10.0).round() / 10.0)
}

const NAMES: [&str; 12] = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// The key: (tonic 0–11 from C, minor?) of mono audio at `rate` Hz.
pub fn key(x: &[f32], rate: u32) -> Option<(usize, bool)> {
    let (n, hop) = (8192, 4096);
    let spec = spectrogram(x, n, hop);
    if spec.is_empty() {
        return None;
    }
    // Each bin's energy to its pitch class, from 55 Hz (A1) to 2 kHz.
    let mut chroma = [0f64; 12];
    for frame in &spec {
        let total: f32 = frame.iter().sum::<f32>().max(1e-9);
        for (b, m) in frame.iter().enumerate().skip(1) {
            let f = b as f64 * rate as f64 / n as f64;
            if !(55.0..=2000.0).contains(&f) {
                continue;
            }
            let midi = 69.0 + 12.0 * (f / 440.0).log2();
            let pc = (midi.round() as i64).rem_euclid(12) as usize;
            // Bins close to a note count more than those between notes.
            let near = 1.0 - 2.0 * (midi - midi.round()).abs();
            chroma[pc] += (*m / total) as f64 * near.max(0.0);
        }
    }
    let corr = |profile: &[f64; 12], tonic: usize| {
        let p: Vec<f64> = (0..12).map(|i| profile[(i + 12 - tonic) % 12]).collect();
        let (mx, mp) = (chroma.iter().sum::<f64>() / 12.0, p.iter().sum::<f64>() / 12.0);
        let num: f64 = (0..12).map(|i| (chroma[i] - mx) * (p[i] - mp)).sum();
        let den = ((0..12).map(|i| (chroma[i] - mx).powi(2)).sum::<f64>() * (0..12).map(|i| (p[i] - mp).powi(2)).sum::<f64>()).sqrt();
        if den > 0.0 { num / den } else { 0.0 }
    };
    let mut best = (0, false, f64::MIN);
    for t in 0..12 {
        for (minor, prof) in [(false, &MAJOR), (true, &MINOR)] {
            let c = corr(prof, t);
            if c > best.2 {
                best = (t, minor, c);
            }
        }
    }
    (best.2 > 0.0).then_some((best.0, best.1))
}

/// "A minor" style name, in the words of `minor_word` / `major_word`.
pub fn key_name(tonic: usize, minor: bool, major_word: &str, minor_word: &str) -> String {
    format!("{} {}", NAMES[tonic % 12], if minor { minor_word } else { major_word })
}

/// Short name, as tags hold it: "Am", "F♯", "B♭m" (written with # and b).
pub fn key_tag(tonic: usize, minor: bool) -> String {
    let n = NAMES[tonic % 12].replace('♯', "#").replace('♭', "b");
    if minor { format!("{n}m") } else { n }
}

/// The Camelot wheel code DJs mix by: 8A = A minor, 8B = C major.
pub fn camelot(tonic: usize, minor: bool) -> String {
    // Major keys from C: 8B, then each fifth up adds one.
    let major_of = |t: usize| ((t * 7) % 12 + 7) % 12 + 1;
    if minor {
        // A minor shares 8 with C major: its relative major is three semitones up.
        format!("{}A", major_of((tonic + 3) % 12))
    } else {
        format!("{}B", major_of(tonic))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_codes() {
        assert_eq!(camelot(0, false), "8B"); // C
        assert_eq!(camelot(9, true), "8A"); // Am
        assert_eq!(camelot(7, false), "9B"); // G
        assert_eq!(camelot(4, true), "9A"); // Em
        assert_eq!(camelot(5, false), "7B"); // F
        assert_eq!(camelot(11, false), "1B"); // B
        assert_eq!(camelot(8, true), "1A"); // G#m
        assert_eq!(key_tag(10, true), "Bbm");
    }

    #[test]
    fn tempo_of_clicks() {
        // A click every 0.5 s (120 BPM), and every 0.6 s (100 BPM), with a quieter off-beat.
        for (period, want) in [(0.5, 120.0), (0.6, 100.0), (0.4, 150.0)] {
            let rate = 22050u32;
            let mut x = vec![0f32; rate as usize * 30];
            let mut t = 0.0;
            while ((t * rate as f64) as usize) < x.len() - 800 {
                let at = (t * rate as f64) as usize;
                for i in 0..400 {
                    x[at + i] += (i as f32 * 0.3).sin() * (1.0 - i as f32 / 400.0);
                }
                t += period;
            }
            let bpm = tempo(&x, rate).unwrap();
            assert!((bpm - want).abs() < 1.5, "{bpm} vs {want}");
        }
    }

    #[test]
    fn key_of_chords() {
        // A minor triad (A, C, E) held, then the same with E major's G# (harmonic minor).
        let rate = 22050u32;
        let note = |midi: f64| 440.0 * 2f64.powf((midi - 69.0) / 12.0);
        let mut x = vec![0f32; rate as usize * 8];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f64 / rate as f64;
            let chord: &[f64] = if t < 4.0 { &[57.0, 60.0, 64.0, 69.0] } else { &[57.0, 62.0, 65.0, 69.0] };
            *s = chord.iter().map(|m| (2.0 * std::f64::consts::PI * note(*m) * t).sin() as f32 * 0.2).sum();
        }
        let (t, minor) = key(&x, rate).unwrap();
        assert_eq!((t, minor), (9, true), "{}", key_name(t, minor, "major", "minor"));
    }
}
