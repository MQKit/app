//! Environment properties given by the parent (contract 9.4): engines, Office, locale.

use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// Folder of an engine: the pinned, downloaded version. Debug builds also accept
/// one found in PATH, for development.
pub fn engine_dir(name: &str) -> Option<PathBuf> {
    if let Some(d) = crate::engines::installed_dir(name) {
        return Some(d);
    }
    if cfg!(debug_assertions) && std::env::var_os("CONVERTOR_NO_PATH_ENGINES").is_none() {
        let exe = format!("{name}.exe");
        let path = std::env::var_os("PATH")?;
        return std::env::split_paths(&path).find(|d| d.join(&exe).is_file());
    }
    None
}

#[cfg(windows)]
fn has_progid(progid: &str) -> bool {
    use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, KEY_READ};
    let key = format!("{progid}\\CLSID");
    let w: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut h = HKEY::default();
    unsafe {
        let ok = RegOpenKeyExW(HKEY_CLASSES_ROOT, windows::core::PCWSTR(w.as_ptr()), Some(0), KEY_READ, &mut h).is_ok();
        if ok {
            let _ = RegCloseKey(h);
        }
        ok
    }
}

#[cfg(not(windows))]
fn has_progid(_: &str) -> bool {
    false
}

/// .NET Framework 4.8 or newer: the Office worker (Word, Excel and PowerPoint through COM)
/// is built for it. Windows 10 1903+ and Windows 11 have it; without it Office is treated
/// as missing, so the tools use LibreOffice instead of failing.
#[cfg(windows)]
fn has_dotnet48() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "SOFTWARE\\Microsoft\\NET Framework Setup\\NDP\\v4\\Full".encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = "Release".encode_utf16().chain(Some(0)).collect();
    let mut release = 0u32;
    let mut len = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(key.as_ptr()),
            windows::core::PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut release as *mut u32 as *mut _),
            Some(&mut len),
        )
        .is_ok()
    };
    // 528040 is the first release number of 4.8 (Microsoft's table of .NET versions).
    ok && release >= 528040
}

#[cfg(not(windows))]
fn has_dotnet48() -> bool {
    false
}

/// An Office program the app can drive: installed, and the worker for it can run.
fn office(progid: &str, dotnet: bool) -> &'static str {
    match (has_progid(progid), dotnet) {
        (true, true) => "ok",
        (true, false) => "no-dotnet",
        _ => "missing",
    }
}

/// Mica is on (Windows 11): the UI leaves the base layer transparent.
pub static MICA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Windows build number (22000+ is Windows 11).
#[cfg(windows)]
pub fn os_build() -> u32 {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion".encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = "CurrentBuildNumber".encode_utf16().chain(Some(0)).collect();
    let mut buf = [0u16; 32];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(key.as_ptr()),
            windows::core::PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        if r.is_err() {
            return 0;
        }
    }
    let n = (len as usize / 2).saturating_sub(1);
    String::from_utf16_lossy(&buf[..n]).trim().parse().unwrap_or(0)
}

#[cfg(not(windows))]
pub fn os_build() -> u32 {
    0
}

pub fn props(locale: &str) -> Value {
    let mut p = Map::new();
    p.insert("mica".into(), json!(MICA.load(std::sync::atomic::Ordering::Relaxed)));
    p.insert("osBuild".into(), json!(os_build()));
    p.insert("debug".into(), json!(cfg!(debug_assertions)));
    for name in crate::engines::catalog().keys() {
        let st = if engine_dir(name).is_some() { "installed" } else { crate::engines::status(name) };
        p.insert(format!("engines.{name}"), json!(st));
    }
    let dotnet = has_dotnet48();
    p.insert("office.word".into(), json!(office("Word.Application", dotnet)));
    p.insert("office.excel".into(), json!(office("Excel.Application", dotnet)));
    p.insert("office.powerpoint".into(), json!(office("PowerPoint.Application", dotnet)));
    p.insert("locale".into(), json!(locale));
    windows_language_features(&mut p);
    Value::Object(p)
}

/// The voices and text recognition languages Windows has, so the tools can list them
/// before any file is added, and say when the user's own language is missing.
#[cfg(windows)]
fn windows_language_features(p: &mut Map<String, Value>) {
    use windows::Globalization::ApplicationLanguages;
    use windows::Media::Ocr::OcrEngine;
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
    let primary = |tag: &str| tag.split('-').next().unwrap_or("").to_ascii_lowercase();
    let own = ApplicationLanguages::Languages().ok().and_then(|l| l.GetAt(0).ok()).map(|t| primary(&t.to_string())).unwrap_or_else(|| "en".into());

    let mut voices = vec![json!({ "value": "", "label": { "ro": "Vocea implicită din Windows", "en": "Windows default voice" } })];
    let mut own_voice = false;
    if let Ok(all) = SpeechSynthesizer::AllVoices() {
        for v in all.into_iter() {
            let (Ok(id), Ok(name), Ok(lang)) = (v.Id(), v.DisplayName(), v.Language()) else { continue };
            own_voice |= primary(&lang.to_string()) == own;
            voices.push(json!({ "value": id.to_string(), "label": format!("{name} ({lang})") }));
        }
    }
    p.insert("windows.voices".into(), json!(voices));
    p.insert("windows.voiceOwnLanguage".into(), json!(own_voice));

    let mut ocr = vec![json!({ "value": "", "label": { "ro": "Automat (limbile din Windows)", "en": "Automatic (your Windows languages)" } })];
    let mut own_ocr = false;
    if let Ok(all) = OcrEngine::AvailableRecognizerLanguages() {
        for l in all.into_iter() {
            let (Ok(tag), Ok(name)) = (l.LanguageTag(), l.DisplayName()) else { continue };
            own_ocr |= primary(&tag.to_string()) == own;
            ocr.push(json!({ "value": tag.to_string(), "label": name.to_string() }));
        }
    }
    p.insert("windows.ocrLanguages".into(), json!(ocr));
    p.insert("windows.ocrOwnLanguage".into(), json!(own_ocr));
}

#[cfg(not(windows))]
fn windows_language_features(_: &mut Map<String, Value>) {}

/// The UI language from Windows: one the app is translated into (`ro`), otherwise `en`.
#[cfg(windows)]
pub fn system_language() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    let s = String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]);
    // The interface's languages (ui/src/lib/i18n.svelte.ts, LANGS); English otherwise.
    let code = s.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    const LANGS: &[&str] = &["en", "ro", "de", "es", "fr", "it", "pt", "pl", "tr", "ru", "ja", "zh"];
    if LANGS.contains(&code.as_str()) { code } else { "en".into() }
}

#[cfg(not(windows))]
pub fn system_language() -> String {
    "en".into()
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn dotnet48_on_this_pc() {
        // Every Windows the tests run on (10 1903+, 11, the CI runners) has 4.8.
        assert!(super::has_dotnet48());
    }
}
