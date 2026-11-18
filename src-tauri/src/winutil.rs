//! Small Windows helpers: Recycle Bin, stopping Office instances started by a worker.

use std::path::Path;

/// The current Windows user's SID ("S-1-5-21-…"), naming that user's work folder.
#[cfg(windows)]
pub fn user_sid() -> String {
    use std::sync::OnceLock;
    static SID: OnceLock<String> = OnceLock::new();
    SID.get_or_init(|| unsafe {
        use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return "user".into();
        }
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), buf.len() as u32, &mut len).is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return "user".into();
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = windows::core::PWSTR::null();
        if ConvertSidToStringSidW(user.User.Sid, &mut s).is_err() {
            return "user".into();
        }
        let out = s.to_string().unwrap_or_else(|_| "user".into());
        let _ = LocalFree(Some(HLOCAL(s.0 as *mut _)));
        out
    })
    .clone()
}

#[cfg(not(windows))]
pub fn user_sid() -> String {
    "user".into()
}

/// Creates a folder only the current user (and the system) may open, not inheriting the
/// drive root's permissions (on C:\ every signed-in user may otherwise read and change
/// it). True if it exists.
#[cfg(windows)]
pub fn create_private_dir(p: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::CreateDirectoryW;
    if p.is_dir() {
        return true;
    }
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{})(A;OICI;FA;;;SY)", user_sid()).encode_utf16().chain(Some(0)).collect();
    let path: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(windows::core::PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None).is_err() {
            return std::fs::create_dir(p).is_ok() || p.is_dir();
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
        let ok = CreateDirectoryW(windows::core::PCWSTR(path.as_ptr()), Some(&sa)).is_ok();
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        ok || p.is_dir()
    }
}

#[cfg(not(windows))]
pub fn create_private_dir(p: &Path) -> bool {
    std::fs::create_dir_all(p).is_ok()
}

/// The user's Documents folder (wherever Windows keeps it, e.g. in OneDrive).
#[cfg(windows)]
pub fn documents_dir() -> std::path::PathBuf {
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            if !s.is_empty() {
                return std::path::PathBuf::from(s);
            }
        }
    }
    std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Documents")
}

#[cfg(not(windows))]
pub fn documents_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Documents")
}

/// Marks a file as downloaded from the internet (its Zone.Identifier, as browsers do):
/// Office then opens a document from a link in Protected View with macros off, and
/// Windows warns before running a program from one.
pub fn mark_from_internet(p: &Path, url: &str) {
    let ads = format!("{}:Zone.Identifier", p.display());
    let _ = std::fs::write(ads, format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={url}\r\n"));
}

/// The mark of an input (if it has one) given to every file of a folder made from it,
/// e.g. an archive's contents: an archive from the internet doesn't lose the warning.
pub fn copy_internet_mark(from: &Path, to_dir: &Path) {
    let Ok(mark) = std::fs::read(format!("{}:Zone.Identifier", from.display())) else { return };
    fn walk(dir: &Path, mark: &[u8]) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), mark),
                Ok(t) if t.is_file() => {
                    let _ = std::fs::write(format!("{}:Zone.Identifier", e.path().display()), mark);
                }
                _ => {}
            }
        }
    }
    walk(to_dir, &mark);
}

/// Whether deleting from `p`'s drive goes to the Recycle Bin. Only fixed local drives have
/// one: on a USB stick, a memory card or a network share, Windows deletes for good.
#[cfg(windows)]
pub fn has_recycle_bin(p: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    let Ok(abs) = std::path::absolute(p) else { return false };
    let wide: Vec<u16> = abs.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut root = [0u16; 1024];
    unsafe {
        if GetVolumePathNameW(windows::core::PCWSTR(wide.as_ptr()), &mut root).is_err() {
            return false;
        }
        // DRIVE_FIXED
        GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) == 3
    }
}

#[cfg(not(windows))]
pub fn has_recycle_bin(_: &Path) -> bool {
    false
}

/// Sends a file to the Recycle Bin, never deleting it for good: on a drive without one
/// it is refused, and a file too large for the bin makes Windows ask first (the person
/// can say no, and the file stays).
#[cfg(windows)]
pub fn recycle(p: &Path) -> Result<(), ()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FO_DELETE, SHFILEOPSTRUCTW};
    if !has_recycle_bin(p) {
        return Err(());
    }
    let abs = std::path::absolute(p).map_err(|_| ())?;
    let mut from: Vec<u16> = abs.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 && !op.fAnyOperationsAborted.as_bool() && !p.exists() {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(not(windows))]
pub fn recycle(_: &Path) -> Result<(), ()> {
    Err(())
}

/// Moves a file to `to` only if nothing is there yet (another job, another program, may
/// have written that name a moment ago): `Ok(false)` when the name is taken. Across
/// drives it copies, then deletes the source.
#[cfg(windows)]
pub fn move_new(from: &Path, to: &Path) -> std::io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_COPY_ALLOWED, MOVEFILE_WRITE_THROUGH};
    let w = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (f, t) = (w(from), w(to));
    match unsafe { MoveFileExW(windows::core::PCWSTR(f.as_ptr()), windows::core::PCWSTR(t.as_ptr()), MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH) } {
        Ok(()) => Ok(true),
        Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() || e.code() == ERROR_FILE_EXISTS.to_hresult() => Ok(false),
        Err(e) => Err(std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF)),
    }
}

#[cfg(not(windows))]
pub fn move_new(from: &Path, to: &Path) -> std::io::Result<bool> {
    if to.exists() {
        return Ok(false);
    }
    std::fs::rename(from, to).map(|_| true)
}

/// When a process started (its creation time), to tell it from a later one that reused
/// its id.
#[cfg(windows)]
pub fn process_started(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        ok.then_some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }
}

#[cfg(not(windows))]
pub fn process_started(_: u32) -> Option<u64> {
    None
}

/// At start: Office programs a worker started before the app crashed or was killed, still
/// running hidden (and keeping someone's document locked). Only the same processes
/// (same id and start time), only Word, Excel or PowerPoint.
pub fn kill_leftover_office() {
    let left: Vec<u32> = crate::state::office_take_all().into_iter().filter(|(pid, started)| process_started(*pid) == Some(*started)).map(|(pid, _)| pid).collect();
    kill_office(&left);
}

/// Stops the Office processes a worker reported (`childProcess`), only if they are
/// still Word/Excel/PowerPoint — never an Office the user opened.
#[cfg(windows)]
pub fn kill_office(pids: &[u32]) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
    for &pid in pids {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { continue };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let mut is_office = false;
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok() {
                let name = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_uppercase();
                is_office = name.ends_with("WINWORD.EXE") || name.ends_with("EXCEL.EXE") || name.ends_with("POWERPNT.EXE");
            }
            if is_office {
                let _ = TerminateProcess(h, 1);
            }
            let _ = CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
pub fn kill_office(_pids: &[u32]) {}
