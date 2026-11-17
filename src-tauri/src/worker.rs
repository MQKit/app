//! Starting plugin processes (contract 12.1): no console window, inside a Job Object
//! (killed with it, lower priority, memory limit), clean environment, stderr to the log.

use crate::registry::{Launch, Tool};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct Proc {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    #[cfg(windows)]
    job: Option<JobHandle>,
}

#[cfg(windows)]
struct JobHandle(windows::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for JobHandle {}
#[cfg(windows)]
unsafe impl Sync for JobHandle {}
#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE: closing the last handle ends every process in the job.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn make_job(child: &Child, memory_mb: u64) -> Option<JobHandle> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;
    use windows::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS;
    unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
        info.BasicLimitInformation.PriorityClass = BELOW_NORMAL_PRIORITY_CLASS.0;
        if memory_mb > 0 {
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.JobMemoryLimit = (memory_mb as usize) * 1024 * 1024;
        }
        let _ = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()));
        Some(JobHandle(job))
    }
}

impl Proc {
    /// Ends the process and everything it started (closes the Job Object).
    pub fn kill(&mut self) {
        #[cfg(windows)]
        {
            self.job = None;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn send(&mut self, msg: &Value) {
        if let Some(s) = self.stdin.as_mut() {
            let _ = writeln!(s, "{msg}");
            let _ = s.flush();
        }
    }
}

/// Starts `tool` with `command`. `temp` becomes TEMP/TMP. Returns the process and its stdout.
pub fn spawn(tool: &Tool, launch: &Launch, command: &str, temp: Option<&Path>, memory_mb: u64) -> std::io::Result<(Proc, ChildStdout)> {
    let mut cmd = Command::new(&launch.exe);
    cmd.args(&launch.args).arg(command).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Clean environment: only what the workers need.
    cmd.env_clear();
    for k in ["SystemRoot", "SystemDrive", "windir", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "PROGRAMDATA", "COMPUTERNAME", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "ProgramFiles", "ProgramFiles(x86)", "CommonProgramFiles", "USERNAME", "HOMEDRIVE", "HOMEPATH"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    let mut path = vec![std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32")];
    for e in &launch.engines {
        if let Some(d) = crate::env::engine_dir(e) {
            // LibreOffice keeps soffice.exe in program/
            if d.join("program").is_dir() {
                path.push(d.join("program"));
            }
            path.push(d);
        }
    }
    for (k, v) in &launch.env {
        cmd.env(k, v);
    }
    cmd.env("PATH", std::env::join_paths(path).unwrap_or_default());
    if let Some(t) = temp {
        let _ = std::fs::create_dir_all(t);
        cmd.env("TEMP", t).env("TMP", t);
    } else if let Some(t) = std::env::var_os("TEMP") {
        cmd.env("TEMP", &t).env("TMP", &t);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    let job = make_job(&child, memory_mb);
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take();
    if let Some(err) = child.stderr.take() {
        let log = crate::paths::logs_dir().join(format!("{}.log", tool.id));
        std::thread::spawn(move || {
            if std::fs::metadata(&log).map(|m| m.len() > 2_000_000).unwrap_or(false) {
                let _ = std::fs::rename(&log, log.with_extension("old.log"));
            }
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&log).ok();
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(f) = f.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
            }
        });
    }
    Ok((Proc { child, stdin, #[cfg(windows)] job }, stdout))
}

/// A short request (`probe`, `analyze`, `preview`): sends it, collects the messages
/// until `done`. Stops the process after `timeout`.
pub fn request(tool: &Tool, launch: &Launch, command: &str, req: &Value, timeout: Duration) -> Result<Vec<Value>, String> {
    let (mut proc, stdout) = spawn(tool, launch, command, None, 0).map_err(|e| e.to_string())?;
    proc.send(req);
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let start = Instant::now();
    let mut out = Vec::new();
    loop {
        let left = timeout.checked_sub(start.elapsed()).unwrap_or_default();
        match rx.recv_timeout(left.max(Duration::from_millis(1))) {
            Ok(v) => {
                let t = v["type"].as_str().unwrap_or("").to_string();
                out.push(v);
                if t == "done" || t == "error" {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                proc.kill();
                return Err("timeout".into());
            }
            Err(_) => break,
        }
    }
    proc.stdin = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = proc.child.try_wait() {
            return Ok(out);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    proc.kill();
    Ok(out)
}
