//! Text in any language: reading files in whatever encoding they were saved with, and
//! the regional settings (code page, list separator) of this PC.

use encoding_rs::Encoding;

/// Bytes → text. Byte order marks first, then UTF-8, then the legacy encoding that
/// fits best (the detector Firefox uses, which knows every common code page).
pub fn decode(raw: &[u8]) -> String {
    if let Some((enc, bom)) = Encoding::for_bom(raw) {
        return enc.decode_without_bom_handling(&raw[bom..]).0.into_owned();
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let mut d = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    d.feed(raw, true);
    d.guess(None, chardetng::Utf8Detection::Allow).decode_without_bom_handling(raw).0.into_owned()
}

/// The encoding older Windows programs use on this PC (its ANSI code page).
pub fn windows_encoding() -> &'static Encoding {
    #[cfg(windows)]
    {
        let acp = unsafe { windows::Win32::Globalization::GetACP() };
        let label = match acp {
            932 => "shift_jis".to_string(),
            936 => "gbk".to_string(),
            949 => "euc-kr".to_string(),
            950 => "big5".to_string(),
            65001 => "utf-8".to_string(),
            n => format!("windows-{n}"),
        };
        if let Some(e) = Encoding::for_label(label.as_bytes()) {
            return e;
        }
    }
    encoding_rs::WINDOWS_1252
}

/// The list separator from the Windows regional settings (what Excel splits CSV on).
pub fn list_separator() -> char {
    locale_char(12 /* LOCALE_SLIST */).unwrap_or(',')
}

/// The decimal separator from the Windows regional settings.
pub fn decimal_separator() -> char {
    locale_char(14 /* LOCALE_SDECIMAL */).unwrap_or('.')
}

/// Does a short date start with the day here (31/12), rather than the month (12/31)?
pub fn day_first() -> bool {
    locale_char(0x1F /* LOCALE_SSHORTDATE */).map(|c| c != 'M').unwrap_or(true)
}

fn locale_char(kind: u32) -> Option<char> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        let mut buf = [0u16; 8];
        // A null locale name means the user's default locale.
        let n = unsafe { windows::Win32::Globalization::GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buf)) };
        if n > 1 {
            return String::from_utf16_lossy(&buf[..(n as usize - 1)]).chars().next();
        }
    }
    let _ = kind;
    None
}
