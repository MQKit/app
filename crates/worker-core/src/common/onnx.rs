//! ONNX Runtime for the AI tools: the DirectML build downloaded as the `onnxruntime`
//! engine (any DirectX 12 graphics card, else the processor), loaded at run time.

use contract::{codes, FileError, Request};
use ort::environment::Environment;
use ort::session::Session;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

static ENV: OnceLock<Result<Arc<Environment>, String>> = OnceLock::new();

fn missing(component: &str) -> FileError {
    FileError::with(codes::ENGINE_MISSING, json!({ "component": component }))
}

/// The folder with onnxruntime.dll and DirectML.dll.
fn runtime_dir(req: &Request) -> Result<PathBuf, FileError> {
    let e = req.engines.get("onnxruntime").ok_or_else(|| missing("onnxruntime"))?;
    let d = e.path.join("site-packages").join("onnxruntime").join("capi");
    if d.join("onnxruntime.dll").is_file() {
        Ok(d)
    } else {
        Err(missing("onnxruntime"))
    }
}

/// A model file of an engine (`engine` folder / `file`).
pub fn model(req: &Request, engine: &str, file: &str) -> Result<PathBuf, FileError> {
    let p = req.engines.get(engine).map(|e| e.path.join(file)).ok_or_else(|| missing(engine))?;
    if p.is_file() {
        Ok(p)
    } else {
        Err(missing(engine))
    }
}

fn environment(dir: &Path) -> Result<Arc<Environment>, FileError> {
    ENV.get_or_init(|| {
        // DirectML.dll from the download, not the older one in System32: loaded first,
        // it is the one onnxruntime.dll gets when it asks for "DirectML.dll".
        #[cfg(windows)]
        unsafe {
            use windows::core::HSTRING;
            let _ = windows::Win32::System::LibraryLoader::LoadLibraryW(&HSTRING::from(dir.join("DirectML.dll").as_os_str()));
        }
        ort::init_from(dir.join("onnxruntime.dll"))
            .map_err(|e| e.to_string())?
            .with_name("convertor")
            .with_telemetry(false)
            .build()
            .map(Arc::new)
            .map_err(|e| e.to_string())
    })
    .clone()
    .map_err(FileError::engine)
}

/// Where a session runs.
pub enum Device {
    Gpu,
    Cpu,
}

/// A session for `model`: on the fastest graphics card when DirectML can use it, else
/// on the processor with `threads` threads.
pub fn session(req: &Request, model: &Path, threads: usize) -> Result<(Session, Device), FileError> {
    let dir = runtime_dir(req)?;
    let env = environment(&dir)?;
    let gpu = || -> Result<Session, String> {
        let s = |e: ort::Error<_>| e.to_string();
        Session::builder(&env).map_err(|e| e.to_string())?
            .with_memory_pattern(false).map_err(s)?
            .with_parallel_execution(false).map_err(s)?
            .with_execution_providers([ort::ep::DirectML::default()
                .with_performance_preference(ort::ep::directml::PerformancePreference::HighPerformance)
                .with_device_filter(ort::ep::directml::DeviceFilter::Gpu)
                .build()
                .error_on_failure()]).map_err(s)?
            .commit_from_file(model).map_err(|e| e.to_string())
    };
    // CONVERTOR_ONNX_CPU=1 skips the graphics card (for comparing).
    if std::env::var_os("CONVERTOR_ONNX_CPU").is_none() {
        if let Ok(s) = gpu() {
            return Ok((s, Device::Gpu));
        }
    }
    Ok((session_cpu(req, model, threads)?, Device::Cpu))
}

/// A session on the processor only: for models DirectML can't run (LaMa's Fourier
/// convolutions stop with a MatMul error on the graphics card).
pub fn session_cpu(req: &Request, model: &Path, threads: usize) -> Result<Session, FileError> {
    let dir = runtime_dir(req)?;
    let env = environment(&dir)?;
    Session::builder(&env).map_err(|e| FileError::engine(e.to_string()))?
        .with_intra_threads(threads.max(1)).map_err(|e| FileError::engine(e.to_string()))?
        .commit_from_file(model).map_err(|e| FileError::engine(e.to_string()))
}
