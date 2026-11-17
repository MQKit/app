//! Small Windows helpers: Recycle Bin, stopping Office instances started by a worker.

use std::path::Path;

#[cfg(windows)]
pub fn recycle(p: &Path) -> Result<(), ()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    let abs = std::path::absolute(p).map_err(|_| ())?;
    let mut from: Vec<u16> = abs.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 && !op.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(not(windows))]
pub fn recycle(p: &Path) -> Result<(), ()> {
    std::fs::remove_file(p).map_err(|_| ())
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
