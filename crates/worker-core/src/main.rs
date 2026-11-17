//! worker-core: the Rust plugins.
//!
//! Usage:
//!   worker-core <plugin-id> <probe|analyze|preview|run|serve>   (request on stdin)
//!   worker-core <plugin-id> <command> --request <file.json>     (development only)
//!   worker-core manifests <dir>                                 (writes <dir>/<id>/plugin.json)

// FileError carries the message shown to the person, in every language: large, and
// returned only when a file fails, so it is not boxed.
#![allow(clippy::result_large_err)]
// Tuples of a few parts read better where they are used than as named types.
#![allow(clippy::type_complexity)]
// Pixels are walked with chunks_exact(3 or 4) everywhere; as_chunks would add a second,
// less familiar spelling for the same loop.
#![allow(clippy::chunks_exact_to_as_chunks)]

mod common;
mod plugins;

use contract::{codes, Cancel, Emitter, FileError, Request};
use plugins::{Ctx, Plugin};
use serde_json::{json, Value};
use std::io::BufRead;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: worker-core <plugin-id> <command> | worker-core manifests <dir>");
        return ExitCode::from(2);
    }
    #[cfg(debug_assertions)]
    if args[1] == "dev-heic" && args.len() >= 5 {
        let o: u16 = args[4].parse().unwrap_or(1);
        return match common::winrt::dev_encode_heic(args[2].as_ref(), args[3].as_ref(), o) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{} {:?}", e.code, e.details);
                ExitCode::from(1)
            }
        };
    }
    // Development only: decodes a HEIC with FFmpeg (from PATH) to PNG, timing it,
    // and prints the PSNR against the Windows decoder when it is available.
    #[cfg(debug_assertions)]
    if args[1] == "dev-heic-decode" && args.len() >= 4 {
        let index: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let req = Request::default();
        let res = (|| -> Result<(), FileError> {
            let ffmpeg = common::ffmpeg::exe(&req, "ffmpeg")?;
            let t0 = std::time::Instant::now();
            let (w, h, px) = common::heic_ffmpeg::decode(&ffmpeg, args[2].as_ref(), index)?;
            println!("ffmpeg decode {w}x{h} in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
            let img = image::RgbaImage::from_raw(w, h, px.clone()).unwrap();
            img.save(&args[3]).map_err(FileError::engine)?;
            if let Ok((ww, wh, wpx)) = common::winrt::decode_image(args[2].as_ref(), index) {
                if (ww, wh) != (w, h) {
                    println!("windows decodes {ww}x{wh}");
                } else {
                    let mse: f64 = px.iter().zip(&wpx).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (a, b))| (*a as f64 - *b as f64).powi(2)).sum::<f64>()
                        / (w as f64 * h as f64 * 3.0);
                    println!("PSNR vs windows: {:.2} dB", 10.0 * (255.0f64 * 255.0 / mse.max(1e-10)).log10());
                }
            }
            Ok(())
        })();
        return match res {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{} {:?}", e.code, e.details);
                ExitCode::from(1)
            }
        };
    }
    if args[1] == "manifests" {
        return write_manifests(&args[2]);
    }
    let Some(plugin) = plugins::find(&args[1]) else {
        eprintln!("unknown plugin {}", args[1]);
        return ExitCode::from(2);
    };
    // Quiet panics: they are caught per file and reported as engine_failed.
    std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));

    let out = Emitter::new();
    out.hello(plugin.id(), plugin.version());

    let cmd = args[2].as_str();
    if cmd == "serve" {
        return serve(plugin, &out);
    }
    let req = match read_request(&args) {
        Ok(r) => r,
        Err(e) => {
            out.error(codes::INVALID_OPTIONS, json!({ "message": e }));
            return ExitCode::from(2);
        }
    };
    let cancel = Cancel::default();
    cancel.watch_stdin();
    match cmd {
        "probe" => {
            out.emit(json!({"type":"env","id":req.id,"props":plugin.probe()}));
            out.done(req.id.as_deref(), None, false);
        }
        "analyze" => analyze(plugin, &req, &out),
        "preview" => preview(plugin, &req, &out),
        "run" => return run(plugin, &req, &out, &cancel),
        other => {
            out.error(codes::INVALID_OPTIONS, json!({ "message": format!("unknown command {other}") }));
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

fn read_request(args: &[String]) -> Result<Request, String> {
    let text = if let Some(i) = args.iter().position(|a| a == "--request") {
        std::fs::read_to_string(args.get(i + 1).ok_or("missing file after --request")?).map_err(|e| e.to_string())?
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        line
    };
    serde_json::from_str(&text).map_err(|e| format!("invalid request: {e}"))
}

fn serve(plugin: &dyn Plugin, out: &Emitter) -> ExitCode {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                out.error(codes::INVALID_OPTIONS, json!({ "message": e.to_string() }));
                continue;
            }
        };
        match req.cmd.as_str() {
            "analyze" => analyze(plugin, &req, out),
            "preview" => preview(plugin, &req, out),
            "probe" => {
                out.emit(json!({"type":"env","id":req.id,"props":plugin.probe()}));
                out.done(req.id.as_deref(), None, false);
            }
            "cancel" => {}
            "quit" => break,
            _ => out.error(codes::INVALID_OPTIONS, json!({ "message": "unsupported in serve" })),
        }
    }
    ExitCode::SUCCESS
}

fn analyze(plugin: &dyn Plugin, req: &Request, out: &Emitter) {
    for f in &req.files {
        let res = catch_unwind(AssertUnwindSafe(|| plugin.analyze(f, req)));
        let a = res.unwrap_or_else(|_| plugins::Analysis::failed(codes::ENGINE_FAILED));
        let mut m = json!({"type":"analysis","id":req.id,"file":f.id,"status":a.status,"props":a.props,"warnings":a.warnings});
        if let Some(c) = a.code {
            m["code"] = json!(c);
        }
        out.emit(m);
    }
    out.done(req.id.as_deref(), None, false);
}

fn preview(plugin: &dyn Plugin, req: &Request, out: &Emitter) {
    let kind = req.target.as_ref().and_then(|t| t.get("kind")).and_then(Value::as_str).unwrap_or("estimate");
    if kind == "estimate" {
        let res = catch_unwind(AssertUnwindSafe(|| plugin.estimate(req)));
        if let Ok(Some(mut est)) = res {
            est["type"] = json!("estimate");
            est["id"] = json!(req.id);
            out.emit(est);
        }
    } else {
        match catch_unwind(AssertUnwindSafe(|| plugin.preview(req))) {
            Ok(Ok(Some(m))) => out.emit(m),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => out.emit(json!({"type":"previewError","id":req.id,"code":e.code,"details":e.details})),
            Err(_) => out.emit(json!({"type":"previewError","id":req.id,"code":codes::ENGINE_FAILED})),
        }
    }
    out.done(req.id.as_deref(), None, false);
}

fn run(plugin: &dyn Plugin, req: &Request, out: &Emitter, cancel: &Cancel) -> ExitCode {
    let (Some(out_dir), Some(work_dir)) = (req.out_dir.clone(), req.work_dir.clone()) else {
        out.error(codes::INVALID_OPTIONS, json!({ "message": "run needs outDir and workDir" }));
        return ExitCode::from(2);
    };
    let _ = std::fs::create_dir_all(&out_dir);
    let _ = std::fs::create_dir_all(&work_dir);
    out.start_heartbeat(std::time::Duration::from_secs(4));
    let ctx = Ctx::new(req, out.clone(), cancel.clone(), out_dir, work_dir);
    let res = catch_unwind(AssertUnwindSafe(|| plugin.run(&ctx)));
    if let Err(_) | Ok(Err(_)) = &res {
        let e = match res {
            Ok(Err(e)) => e,
            _ => FileError::new(codes::ENGINE_FAILED),
        };
        out.error(&e.code, e.details.unwrap_or(Value::Null));
        return ExitCode::from(1);
    }
    out.done(None, Some(ctx.summary()), cancel.is_cancelled());
    ExitCode::SUCCESS
}

fn write_manifests(dir: &str) -> ExitCode {
    for p in plugins::all().into_iter().filter(|p| !p.hidden()) {
        let d = std::path::Path::new(dir).join(p.id());
        if let Err(e) = std::fs::create_dir_all(&d) {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
        let mut m = p.manifest();
        m["protocol"] = json!(contract::PROTOCOL);
        m["id"] = json!(p.id());
        m["version"] = json!(p.version());
        if m.get("entry").is_none() {
            m["entry"] = json!([{ "kind": "worker", "worker": "worker-core", "command": p.id() }]);
        }
        let text = serde_json::to_string_pretty(&m).unwrap();
        if let Err(e) = std::fs::write(d.join("plugin.json"), text) {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
        println!("{}", p.id());
    }
    ExitCode::SUCCESS
}
