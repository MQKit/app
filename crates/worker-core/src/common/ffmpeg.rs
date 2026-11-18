//! FFmpeg wrapper shared by every FFmpeg tool: locating the engine, probing,
//! running with progress from `-progress pipe:1`, and cancellation.

use crate::plugins::Ctx;
use contract::{codes, FileError, Request};
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `ffmpeg.exe` / `ffprobe.exe`: from `engines.ffmpeg.path` (a folder), else from PATH.
pub fn exe(req: &Request, name: &str) -> Result<PathBuf, FileError> {
    if let Some(e) = req.engines.get("ffmpeg") {
        let p = e.path.join(format!("{name}.exe"));
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(format!("{name}.exe"));
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(FileError::with(codes::ENGINE_MISSING, json!({ "component": name })))
}

pub(crate) fn command(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// ffprobe as JSON → the media properties of the contract (section 9.5).
pub fn probe(req: &Request, input: &Path) -> Result<Map<String, Value>, FileError> {
    let mut child = command(&exe(req, "ffprobe")?)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "ffprobe", "message": e.to_string() })))?;
    // Reading the headers takes a second; a damaged file can make ffprobe search for long.
    let limit = Duration::from_secs(90);
    let (status, stdout, stderr) = crate::common::proc::output(&mut child, "ffprobe", limit)?;
    if !status.success() {
        let msg = String::from_utf8_lossy(&stderr).trim().to_string();
        // A recording that was cut off (the phone or camera stopped before closing the file).
        // A file named like audio or video that holds none is a damaged one.
        let ext = input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        const MEDIA: &[&str] = &["mp4", "m4v", "mov", "mkv", "avi", "webm", "wmv", "flv", "3gp", "mpg", "mpeg", "ts", "mts", "m2ts",
                                 "mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma", "aiff", "aif"];
        let garbled = msg.contains("Invalid data found") && MEDIA.contains(&ext.as_str());
        let code = if garbled || msg.contains("moov atom not found") || msg.contains("partial file") { codes::CORRUPT_FILE } else { codes::UNSUPPORTED_FILE };
        return Err(FileError::with(code, json!({ "message": msg })));
    }
    let v: Value = serde_json::from_slice(&stdout).map_err(FileError::engine)?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let mut p = Map::new();
    let dur = v["format"]["duration"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    p.insert("durationSec".into(), json!((dur * 1000.0).round() / 1000.0));
    let video: Vec<&Value> = streams
        .iter()
        .filter(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64() != Some(1))
        .collect();
    let audio: Vec<&Value> = streams.iter().filter(|s| s["codec_type"] == "audio").collect();
    p.insert("hasVideo".into(), json!(!video.is_empty()));
    p.insert("hasAudio".into(), json!(!audio.is_empty()));
    if let Some(vs) = video.first() {
        let fps = vs["avg_frame_rate"].as_str().or(vs["r_frame_rate"].as_str()).and_then(|r| {
            let (a, b) = r.split_once('/')?;
            let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
            (b > 0.0 && a > 0.0).then(|| ((a / b) * 100.0).round() / 100.0)
        });
        let hdr = matches!(vs["color_transfer"].as_str(), Some("smpte2084") | Some("arib-std-b67"));
        // Phones store a portrait video as landscape plus a rotation: the size as shown
        // (FFmpeg turns the picture before any filter sees it).
        let rotation = vs["side_data_list"]
            .as_array()
            .and_then(|l| l.iter().find_map(|d| d["rotation"].as_f64()))
            .or_else(|| vs["tags"]["rotate"].as_str().and_then(|r| r.parse::<f64>().ok()))
            .unwrap_or(0.0);
        let quarter = ((rotation / 90.0).round() as i64).rem_euclid(2) == 1;
        let (w, h) = if quarter { (vs["height"].clone(), vs["width"].clone()) } else { (vs["width"].clone(), vs["height"].clone()) };
        p.insert("video".into(), json!({ "codec": vs["codec_name"], "width": w, "height": h, "fps": fps, "hdr": hdr, "rotation": rotation, "pixFmt": vs["pix_fmt"],
                                          "transfer": vs["color_transfer"], "primaries": vs["color_primaries"], "matrix": vs["color_space"] }));
        p.insert("width".into(), w);
        p.insert("height".into(), h);
    }
    let tracks: Vec<Value> = audio
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let lang = a["tags"]["language"].as_str().unwrap_or("");
            let title = a["tags"]["title"].as_str().unwrap_or("");
            let ch = a["channels"].as_i64().unwrap_or(0);
            let codec = a["codec_name"].as_str().unwrap_or("");
            let mut label = format!("{}", i + 1);
            if !title.is_empty() {
                label = format!("{label}. {title}");
            } else if !lang.is_empty() && lang != "und" {
                label = format!("{label}. {lang}");
            }
            // Bit depth of the samples (lossless sources): 16, 24 or 32.
            let bits = a["bits_per_raw_sample"]
                .as_str()
                .and_then(|b| b.parse::<i64>().ok())
                .filter(|b| *b > 0)
                .or_else(|| a["bits_per_sample"].as_i64().filter(|b| *b > 0))
                .or_else(|| match a["sample_fmt"].as_str() {
                    Some("s32" | "s32p" | "flt" | "fltp" | "dbl" | "dblp") if matches!(codec, "pcm_f32le" | "pcm_s32le" | "flac" | "alac" | "pcm_f64le") => Some(24),
                    _ => None,
                })
                .unwrap_or(16);
            json!({
                "value": i, "label": label, "detail": format!("{codec}, {ch} ch"),
                "index": i, "codec": codec, "channels": ch, "language": lang, "title": title,
                "sampleRate": a["sample_rate"].as_str().and_then(|r| r.parse::<i64>().ok()).unwrap_or(0), "bits": bits,
                "default": a["disposition"]["default"].as_i64() == Some(1)
            })
        })
        .collect();
    p.insert("audioTracks".into(), json!(tracks));
    let subs: Vec<Value> = streams
        .iter()
        .filter(|s| s["codec_type"] == "subtitle")
        .enumerate()
        .map(|(i, s)| json!({ "index": i, "codec": s["codec_name"], "language": s["tags"]["language"] }))
        .collect();
    p.insert("subtitleTracks".into(), json!(subs));
    if let Some(br) = v["format"]["bit_rate"].as_str().and_then(|s| s.parse::<f64>().ok()) {
        p.insert("bitRate".into(), json!(br.round()));
    }
    p.insert("hasAttachedPic".into(), json!(streams.iter().any(|s| s["disposition"]["attached_pic"].as_i64() == Some(1))));
    Ok(p)
}

/// Runs ffmpeg with `args` (input/output included), reporting progress for `file`
/// against `expected_sec` of output. Kills ffmpeg on cancel.
pub fn run(ctx: &Ctx, file: &str, args: &[String], expected_sec: f64, stage: u32, stages: u32) -> Result<(), FileError> {
    let mut child = command(&exe(ctx.req, "ffmpeg")?)
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-progress", "pipe:1", "-y"])
        .args(["-threads", &ctx.req.limits.threads.max(1).to_string()])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FileError::with(codes::ENGINE_MISSING, json!({ "component": "ffmpeg", "message": e.to_string() })))?;

    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let (tx, rx) = std::sync::mpsc::channel::<f64>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let _ = tx.send(us / 1e6);
                }
            }
        }
    });

    loop {
        if ctx.cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FileError::new(codes::CANCELLED));
        }
        while let Ok(sec) = rx.try_recv() {
            if expected_sec > 0.0 {
                ctx.progress(file, (sec / expected_sec).min(0.99), "encode", stage, stages);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let err = err_thread.join().unwrap_or_default();
                if status.success() {
                    return Ok(());
                }
                let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                eprintln!("ffmpeg: {err}");
                let code = if err.contains("No space left") { codes::DISK_FULL } else { codes::ENGINE_FAILED };
                return Err(FileError::with(code, json!({ "exitCode": status.code(), "log": tail })));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(crate::plugins::io_err(e)),
        }
    }
}

/// Whether the probed video is HDR (HDR10/PQ or HLG, what phones record).
pub fn is_hdr(info: &Map<String, Value>) -> bool {
    info.get("video").and_then(|v| v["hdr"].as_bool()).unwrap_or(false)
}

/// Filters that turn HDR into ordinary (SDR, BT.709) pixels, for 8-bit outputs: without
/// them an HDR phone video comes out grey and washed out. Ends in `format=<pix_fmt>`.
pub fn to_sdr(pix_fmt: &str) -> String {
    format!("zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=mobius,zscale=t=bt709:m=bt709:r=tv,format={pix_fmt}")
}

/// Encoder arguments that keep an HDR video HDR: 10-bit HEVC (or VP9 for WebM) tagged
/// with the source's colours, so phones and TVs show it as recorded.
pub fn keep_hdr(info: &Map<String, Value>, webm: bool, crf: u32) -> Vec<String> {
    let v = info.get("video").cloned().unwrap_or(Value::Null);
    let tag = |k: &str, d: &str| v[k].as_str().filter(|s| !s.is_empty() && *s != "unknown").unwrap_or(d).to_string();
    let (trc, prim, mat) = (tag("transfer", "arib-std-b67"), tag("primaries", "bt2020"), tag("matrix", "bt2020nc"));
    let mut a: Vec<String> = if webm {
        ["-c:v", "libvpx-vp9", "-profile:v", "2", "-pix_fmt", "yuv420p10le", "-b:v", "0", "-row-mt", "1", "-crf"].iter().map(|s| s.to_string()).collect()
    } else {
        ["-c:v", "libx265", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1", "-crf"].iter().map(|s| s.to_string()).collect()
    };
    a.push(crf.to_string());
    if !webm {
        a.push("-x265-params".into());
        a.push(format!("log-level=error:colorprim={prim}:transfer={trc}:colormatrix={mat}"));
    }
    a.extend(["-color_primaries".into(), prim, "-color_trc".into(), trc, "-colorspace".into(), mat]);
    a
}

/// Output colour tags for SDR video.
pub fn sdr_tags() -> Vec<String> {
    ["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709"].iter().map(|s| s.to_string()).collect()
}

/// "hh:mm:ss.mmm", "mm:ss" or seconds → seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for part in s.split(':') {
        total = total * 60.0 + part.replace(',', ".").parse::<f64>().ok()?;
    }
    Some(total)
}
