//! Opens password-protected Office documents (.docx, .xlsx, .pptx…) saved by Office 2010
//! and later ("agile" encryption, MS-OFFCRYPTO 2.3.4.10): an OLE file holding
//! `EncryptionInfo` (an XML description) and `EncryptedPackage` (the real file, AES-CBC
//! in 4096-byte segments). LibreOffice's command line can't take a password, so the
//! document is decrypted to a private copy first.

use std::io::Read;

pub enum Error {
    /// Not an encrypted OOXML package (a plain file, or an older kind of protection).
    Unsupported,
    WrongPassword,
    Corrupt,
}

/// Whether `path` is an Office document encrypted with a password (OLE with an
/// `EncryptedPackage` stream).
pub fn is_encrypted(path: &std::path::Path) -> bool {
    cfb::open(path).map(|c| c.exists("EncryptedPackage") && c.exists("EncryptionInfo")).unwrap_or(false)
}

/// An Excel 97-2003 workbook or a PowerPoint 97-2003 presentation with a password: the
/// workbook's FILEPASS record (right after its first BOF) or the presentation's "encrypted"
/// CurrentUser token. These can't be checked or opened here; Office opens them.
pub fn legacy_encrypted(path: &std::path::Path) -> bool {
    let Ok(mut c) = cfb::open(path) else { return false };
    if let Ok(mut s) = c.open_stream("Workbook").or_else(|_| c.open_stream("Book")) {
        let mut head = vec![0u8; 4096];
        let n = s.read(&mut head).unwrap_or(0);
        let mut i = 0;
        for _ in 0..40 {
            if i + 4 > n {
                break;
            }
            let kind = u16::from_le_bytes([head[i], head[i + 1]]);
            let len = u16::from_le_bytes([head[i + 2], head[i + 3]]) as usize;
            if kind == 0x002F {
                return true;
            }
            if kind == 0x000A {
                break;
            }
            i += 4 + len;
        }
        return false;
    }
    if let Ok(mut s) = c.open_stream("Current User") {
        let mut head = [0u8; 16];
        if s.read_exact(&mut head).is_ok() {
            return u32::from_le_bytes([head[12], head[13], head[14], head[15]]) == 0xF3D1_C4DF;
        }
    }
    false
}

/// What the analysis of a password-protected Office file says once a password is typed:
/// `Ok(Some(zip))` when it opens, `Ok(None)` when it can't be checked here (old formats:
/// the conversion checks it), or the error code (`wrong_password`, `corrupt_file`).
pub fn unlock(path: &std::path::Path, password: &str) -> Result<Option<Vec<u8>>, &'static str> {
    match decrypt(path, password) {
        Ok(data) => Ok(Some(data)),
        Err(Error::WrongPassword) => Err(contract::codes::WRONG_PASSWORD),
        Err(Error::Corrupt) => Err(contract::codes::CORRUPT_FILE),
        Err(Error::Unsupported) => Ok(None),
    }
}

/// The decrypted document (a zip: .docx, .xlsx…).
pub fn decrypt(path: &std::path::Path, password: &str) -> Result<Vec<u8>, Error> {
    let mut c = cfb::open(path).map_err(|_| Error::Unsupported)?;
    let mut info = Vec::new();
    c.open_stream("EncryptionInfo").map_err(|_| Error::Unsupported)?.read_to_end(&mut info).map_err(|_| Error::Corrupt)?;
    let mut package = Vec::new();
    c.open_stream("EncryptedPackage").map_err(|_| Error::Unsupported)?.read_to_end(&mut package).map_err(|_| Error::Corrupt)?;
    // Version 4.4 is agile; 3.2/4.2 ("standard", Office 2007) is not handled here.
    if info.len() < 8 || info[0..4] != [4, 0, 4, 0] {
        return Err(Error::Unsupported);
    }
    let xml = String::from_utf8_lossy(&info[8..]).to_string();
    let key_data = element(&xml, "keyData").ok_or(Error::Corrupt)?;
    let encrypted_key = element(&xml, "p:encryptedKey").or_else(|| element(&xml, "encryptedKey")).ok_or(Error::Corrupt)?;

    let b64 = |s: &str| base64_decode(s).ok_or(Error::Corrupt);
    let num = |el: &str, name: &str| attr(el, name).and_then(|v| v.parse::<usize>().ok()).ok_or(Error::Corrupt);
    let hash = Hash::from(&attr(&encrypted_key, "hashAlgorithm").ok_or(Error::Corrupt)?).ok_or(Error::Unsupported)?;
    if attr(&encrypted_key, "cipherAlgorithm").as_deref() != Some("AES") || attr(&encrypted_key, "cipherChaining").as_deref() != Some("ChainingModeCBC") {
        return Err(Error::Unsupported);
    }
    let salt = b64(&attr(&encrypted_key, "saltValue").ok_or(Error::Corrupt)?)?;
    let spins = num(&encrypted_key, "spinCount")?.min(10_000_000);
    let key_bytes = num(&encrypted_key, "keyBits")? / 8;
    let hash_size = num(&encrypted_key, "hashSize")?;

    // The password hash, spun `spinCount` times.
    let pw: Vec<u8> = password.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut h = hash.digest(&[&salt, &pw]);
    for i in 0..spins as u32 {
        h = hash.digest(&[&i.to_le_bytes(), &h]);
    }
    let derive = |block: &[u8]| -> Vec<u8> {
        let mut k = hash.digest(&[&h, block]);
        k.resize(key_bytes, 0x36);
        k
    };
    const VERIFIER_INPUT: [u8; 8] = [0xfe, 0xa7, 0xd2, 0x76, 0x3b, 0x4b, 0x9e, 0x79];
    const VERIFIER_HASH: [u8; 8] = [0xd7, 0xaa, 0x0f, 0x6d, 0x30, 0x61, 0x34, 0x4e];
    const KEY_VALUE: [u8; 8] = [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6];
    let iv = |s: &[u8]| -> Vec<u8> { s[..16.min(s.len())].to_vec() };

    // The password is right when the verifier's hash matches.
    let input = aes_cbc(&derive(&VERIFIER_INPUT), &iv(&salt), &b64(&attr(&encrypted_key, "encryptedVerifierHashInput").ok_or(Error::Corrupt)?)?)?;
    let salt_size = num(&encrypted_key, "saltSize")?.min(input.len());
    let expected = aes_cbc(&derive(&VERIFIER_HASH), &iv(&salt), &b64(&attr(&encrypted_key, "encryptedVerifierHashValue").ok_or(Error::Corrupt)?)?)?;
    let got = hash.digest(&[&input[..salt_size]]);
    if expected.len() < hash_size || got.len() < hash_size || expected[..hash_size] != got[..hash_size] {
        return Err(Error::WrongPassword);
    }
    let mut secret = aes_cbc(&derive(&KEY_VALUE), &iv(&salt), &b64(&attr(&encrypted_key, "encryptedKeyValue").ok_or(Error::Corrupt)?)?)?;
    let data_key_bytes = num(&key_data, "keyBits")? / 8;
    secret.truncate(data_key_bytes);
    let data_hash = Hash::from(&attr(&key_data, "hashAlgorithm").ok_or(Error::Corrupt)?).ok_or(Error::Unsupported)?;
    let data_salt = b64(&attr(&key_data, "saltValue").ok_or(Error::Corrupt)?)?;

    // The package: its size, then 4096-byte segments, each with its own IV.
    if package.len() < 8 {
        return Err(Error::Corrupt);
    }
    let size = u64::from_le_bytes(package[0..8].try_into().unwrap()) as usize;
    let mut out = Vec::with_capacity(size + 4096);
    for (i, seg) in package[8..].chunks(4096).enumerate() {
        let seg_iv = data_hash.digest(&[&data_salt, &(i as u32).to_le_bytes()]);
        let mut seg = seg.to_vec();
        seg.truncate(seg.len() / 16 * 16);
        out.extend(aes_cbc(&secret, &seg_iv[..16], &seg)?);
    }
    if out.len() < size {
        return Err(Error::Corrupt);
    }
    out.truncate(size);
    if !out.starts_with(b"PK") {
        return Err(Error::WrongPassword);
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn from(name: &str) -> Option<Hash> {
        Some(match name {
            "SHA1" => Hash::Sha1,
            "SHA256" => Hash::Sha256,
            "SHA384" => Hash::Sha384,
            "SHA512" => Hash::Sha512,
            _ => return None,
        })
    }

    fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        use sha2::Digest;
        macro_rules! run {
            ($t:ty) => {{
                let mut h = <$t>::new();
                for p in parts {
                    h.update(p);
                }
                h.finalize().to_vec()
            }};
        }
        match self {
            Hash::Sha1 => run!(sha1::Sha1),
            Hash::Sha256 => run!(sha2::Sha256),
            Hash::Sha384 => run!(sha2::Sha384),
            Hash::Sha512 => run!(sha2::Sha512),
        }
    }
}

/// AES-CBC decryption without padding (the data is a whole number of blocks).
fn aes_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    use cbc::cipher::{BlockModeDecrypt, KeyIvInit};
    let data = &data[..data.len() / 16 * 16];
    let mut buf = data.to_vec();
    macro_rules! dec {
        ($aes:ty) => {{
            let d = cbc::Decryptor::<$aes>::new_from_slices(key, iv).map_err(|_| Error::Corrupt)?;
            d.decrypt_padded::<cbc::cipher::block_padding::NoPadding>(&mut buf).map_err(|_| Error::Corrupt)?;
        }};
    }
    match key.len() {
        16 => dec!(aes::Aes128),
        24 => dec!(aes::Aes192),
        32 => dec!(aes::Aes256),
        _ => return Err(Error::Unsupported),
    }
    Ok(buf)
}

/// The start tag of the first `<name …>` element, attributes included.
fn element(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{name} "))?;
    let end = xml[start..].find('>')? + start;
    Some(xml[start..end].to_string())
}

fn attr(el: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let i = el.find(&key)? + key.len();
    let j = el[i..].find('"')? + i;
    Some(el[i..j].to_string())
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64() {
        assert_eq!(super::base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(super::base64_decode("AAEC").unwrap(), [0, 1, 2]);
    }

    /// A .docx Word protected with the password "secret123" (made by the test setup).
    #[test]
    fn word_document() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/out/dl/doc/parola.docx");
        if !p.exists() {
            return;
        }
        assert!(super::is_encrypted(&p));
        assert!(matches!(super::decrypt(&p, "wrong"), Err(super::Error::WrongPassword)));
        let zip = super::decrypt(&p, "secret123").ok().expect("decrypts");
        let z = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        assert!(z.file_names().any(|n| n.map(|n| n == "word/document.xml").unwrap_or(false)));
    }
}
