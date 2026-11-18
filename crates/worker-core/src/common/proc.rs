//! Waiting for external programs (LibreOffice, Ghostscript, FFmpeg): cancel stops them,
//! and so does a time limit, so a program that hangs never keeps the job alive (the
//! heartbeat repeats the last progress for as long as the worker waits).

use contract::{codes, Cancel, FileError};
use serde_json::json;
use std::io::Read;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// A generous time limit for a program working on `bytes` of input: `base`, plus
/// `per_mb` for every MB, at most `max`.
pub fn limit_for(bytes: u64, base: Duration, per_mb: Duration, max: Duration) -> Duration {
    let mb = bytes.div_ceil(1_000_000).min(1_000_000) as u32;
    (base + per_mb * mb).min(max)
}

/// The error for a program stopped after `limit`.
pub fn timed_out(component: &str, limit: Duration) -> FileError {
    FileError::with(codes::TIMEOUT, json!({ "component": component, "seconds": limit.as_secs() }))
}

/// Waits for `child`, calling `tick` about five times a second (progress). On cancel it
/// kills the program and returns `cancelled`; after `limit`, `timeout`.
pub fn wait(child: &mut Child, cancel: Option<&Cancel>, component: &str, limit: Duration, mut tick: impl FnMut()) -> Result<ExitStatus, FileError> {
    let started = Instant::now();
    loop {
        if cancel.is_some_and(Cancel::is_cancelled) {
            kill(child);
            return Err(FileError::new(codes::CANCELLED));
        }
        match child.try_wait() {
            Ok(Some(s)) => return Ok(s),
            Ok(None) => {}
            Err(e) => {
                kill(child);
                return Err(crate::plugins::io_err(e));
            }
        }
        if started.elapsed() >= limit {
            kill(child);
            eprintln!("{component}: stopped after {} s", limit.as_secs());
            return Err(timed_out(component, limit));
        }
        tick();
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Like `Command::output`, with a time limit: the program's exit status, stdout and stderr.
/// Needs stdout and stderr piped.
pub fn output(child: &mut Child, component: &str, limit: Duration) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), FileError> {
    let reader = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut v);
            }
            v
        })
    };
    let out = reader(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = reader(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let status = wait(child, None, component, limit, || {})?;
    Ok((status, out.join().unwrap_or_default(), err.join().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_grow_with_size_up_to_the_cap() {
        let (b, m, x) = (Duration::from_secs(60), Duration::from_secs(2), Duration::from_secs(600));
        assert_eq!(limit_for(0, b, m, x), b);
        assert_eq!(limit_for(10_000_000, b, m, x), Duration::from_secs(80));
        assert_eq!(limit_for(u64::MAX, b, m, x), x);
    }

    #[cfg(windows)]
    #[test]
    fn a_hung_program_is_stopped() {
        let mut c = std::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let t = Instant::now();
        let e = wait(&mut c, None, "test", Duration::from_millis(500), || {}).unwrap_err();
        assert_eq!(e.code, codes::TIMEOUT);
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[cfg(windows)]
    #[test]
    fn cancel_stops_a_program() {
        let cancel = Cancel::default();
        cancel.cancel();
        let mut c = std::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let e = wait(&mut c, Some(&cancel), "test", Duration::from_secs(60), || {}).unwrap_err();
        assert_eq!(e.code, codes::CANCELLED);
    }
}
