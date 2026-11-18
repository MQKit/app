//! Base64, for pictures carried inside SVG files and inside the options (a signature
//! drawn in the app arrives as a data: URL).

const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(v >> 18) as usize & 63] as char);
        out.push(T[(v >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[v as usize & 63] as char } else { '=' });
    }
    out
}

/// Standard or URL-safe alphabet; spaces and line breaks skipped; None if not base64.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        if c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The bytes of a `data:…;base64,…` URL.
pub fn data_url(s: &str) -> Option<Vec<u8>> {
    let (head, body) = s.strip_prefix("data:")?.split_once(',')?;
    head.ends_with(";base64").then(|| decode(body)).flatten()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        for s in [&b""[..], b"M", b"Ma", b"Man", b"\x00\xff\x10hello"] {
            assert_eq!(super::decode(&super::encode(s)).unwrap(), s);
        }
        assert_eq!(super::data_url("data:image/png;base64,TWE=").unwrap(), b"Ma");
        assert!(super::data_url("data:text/plain,hi").is_none());
        assert!(super::decode("@@").is_none());
    }
}
