//! The text inside Outlook's RTF bodies. Outlook often keeps a mail's HTML only
//! wrapped in RTF ("encapsulated HTML", MS-OXRTFEX): the HTML comes back out of the
//! `\*\htmltag` groups and the text outside `\htmlrtf` blocks. Plain RTF gives plain text.

/// What the RTF holds: the original HTML, or text.
pub enum Body {
    Html(String),
    Text(String),
}

pub fn extract(rtf: &[u8]) -> Body {
    let html = contains(rtf, b"\\fromhtml1");
    let mut p = Parser { s: rtf, i: 0, out: Vec::new(), codepage: codepage(rtf), uc: 1, html };
    p.run();
    let text = decode(&p.out, p.codepage);
    if html {
        Body::Html(text)
    } else {
        Body::Text(text)
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `\ansicpgNNNN`, else Windows-1252.
fn codepage(rtf: &[u8]) -> u16 {
    let key = b"\\ansicpg";
    rtf.windows(key.len()).position(|w| w == key).and_then(|i| {
        let digits: String = rtf[i + key.len()..].iter().take_while(|b| b.is_ascii_digit()).map(|&b| b as char).collect();
        digits.parse().ok()
    }).unwrap_or(1252)
}

/// Output bytes: plain bytes are in the code page; `\u` letters are pushed as UTF-8 behind
/// a marker so the two can be told apart when decoding.
const UTF8_MARK: u8 = 0x00;

fn decode(out: &[u8], codepage: u16) -> String {
    let enc = codepage_encoding(codepage);
    let mut s = String::new();
    let mut i = 0;
    let mut run = Vec::new();
    while i < out.len() {
        if out[i] == UTF8_MARK && i + 1 < out.len() {
            let n = out[i + 1] as usize;
            s.push_str(&enc.decode(&run).0);
            run.clear();
            s.push_str(&String::from_utf8_lossy(&out[i + 2..(i + 2 + n).min(out.len())]));
            i += 2 + n;
        } else {
            run.push(out[i]);
            i += 1;
        }
    }
    s.push_str(&enc.decode(&run).0);
    s
}

fn codepage_encoding(cp: u16) -> &'static encoding_rs::Encoding {
    let label = match cp {
        65001 => "utf-8".to_string(),
        932 => "shift_jis".into(),
        936 => "gbk".into(),
        949 => "euc-kr".into(),
        950 => "big5".into(),
        20866 => "koi8-r".into(),
        28591..=28599 => format!("iso-8859-{}", cp - 28590),
        n => format!("windows-{n}"),
    };
    encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
}

struct Group {
    skip: bool,
    htmltag: bool,
    htmlrtf: bool,
    uc: usize,
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    out: Vec<u8>,
    codepage: u16,
    uc: usize,
    html: bool,
}

impl Parser<'_> {
    fn emit(&mut self, stack: &[Group], b: &[u8]) {
        let g = stack.last();
        if g.is_some_and(|g| g.skip) {
            return;
        }
        if self.html && g.is_some_and(|g| g.htmlrtf && !g.htmltag) {
            return;
        }
        self.out.extend_from_slice(b);
    }

    fn emit_char(&mut self, stack: &[Group], c: char) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        let mut b = vec![UTF8_MARK, s.len() as u8];
        b.extend_from_slice(s.as_bytes());
        self.emit(stack, &b);
    }

    fn run(&mut self) {
        let mut stack = vec![Group { skip: false, htmltag: false, htmlrtf: false, uc: 1 }];
        let mut skip_chars = 0usize;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            match c {
                b'{' => {
                    let top = stack.last().map(|g| Group { skip: g.skip, htmltag: g.htmltag, htmlrtf: g.htmlrtf, uc: g.uc }).unwrap();
                    stack.push(top);
                    self.i += 1;
                    // `{\*\destination …}` groups are skipped unless we know them.
                    if self.s[self.i..].starts_with(b"\\*") {
                        self.i += 2;
                        let word = self.peek_word();
                        let g = stack.last_mut().unwrap();
                        if self.html && word.starts_with(b"htmltag") {
                            g.htmltag = true;
                            g.skip = false;
                        } else {
                            g.skip = true;
                        }
                    }
                }
                b'}' => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    self.i += 1;
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&n) = self.s.get(self.i) else { break };
                    if n == b'\'' {
                        let hex = std::str::from_utf8(self.s.get(self.i + 1..self.i + 3).unwrap_or(b"")).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                        self.i += 3;
                        if skip_chars > 0 {
                            skip_chars -= 1;
                        } else if let Some(b) = hex {
                            self.emit(&stack, &[b]);
                        }
                        continue;
                    }
                    if !n.is_ascii_alphabetic() {
                        // Escaped symbols: \\ \{ \} and the special ~ - _
                        self.i += 1;
                        match n {
                            b'\\' | b'{' | b'}' => self.emit(&stack, &[n]),
                            b'~' => self.emit_char(&stack, '\u{a0}'),
                            b'_' => self.emit(&stack, b"-"),
                            b'\r' | b'\n' => self.emit(&stack, b"\r\n"),
                            _ => {}
                        }
                        continue;
                    }
                    let (word, param) = self.read_word();
                    let g = stack.last_mut().unwrap();
                    match word.as_slice() {
                        b"htmlrtf" => g.htmlrtf = param != Some(0),
                        b"uc" => g.uc = param.unwrap_or(1).max(0) as usize,
                        b"u" => {
                            let mut v = param.unwrap_or(0);
                            if v < 0 {
                                v += 65536;
                            }
                            if let Some(ch) = char::from_u32(v as u32) {
                                self.emit_char(&stack, ch);
                            }
                            skip_chars = stack.last().map(|g| g.uc).unwrap_or(1);
                        }
                        b"par" | b"line" => self.emit(&stack, b"\r\n"),
                        b"tab" => self.emit(&stack, b"\t"),
                        b"fonttbl" | b"colortbl" | b"stylesheet" | b"info" | b"pict" | b"header" | b"footer" | b"listtable" | b"listoverridetable" => g.skip = true,
                        _ => {}
                    }
                    self.uc = stack.last().map(|g| g.uc).unwrap_or(1);
                }
                b'\r' | b'\n' => self.i += 1,
                _ => {
                    self.i += 1;
                    if skip_chars > 0 {
                        skip_chars -= 1;
                    } else {
                        self.emit(&stack, &[c]);
                    }
                }
            }
        }
    }

    fn peek_word(&self) -> Vec<u8> {
        let mut j = self.i;
        if self.s.get(j) == Some(&b'\\') {
            j += 1;
        }
        self.s[j..].iter().take_while(|b| b.is_ascii_alphabetic()).copied().collect()
    }

    /// A control word and its number; one space after it belongs to it.
    fn read_word(&mut self) -> (Vec<u8>, Option<i32>) {
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
            self.i += 1;
        }
        let word = self.s[start..self.i].to_vec();
        let num_start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        let param = std::str::from_utf8(&self.s[num_start..self.i]).ok().and_then(|n| n.parse().ok());
        if self.s.get(self.i) == Some(&b' ') {
            self.i += 1;
        }
        (word, param)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encapsulated_html() {
        let rtf = br"{\rtf1\ansi\ansicpg1250\fromhtml1 {\fonttbl{\f0 Arial;}}{\*\htmltag19 <html>}{\*\htmltag50 <body>}\htmlrtf {\htmlrtf0 Bun\'e3 ziua \u537?i pa\par{\*\htmltag84 <b>}tare{\*\htmltag92 </b>}\htmlrtf }\htmlrtf0 {\*\htmltag58 </body>}}";
        match extract(rtf) {
            Body::Html(h) => {
                assert!(h.contains("<html>") && h.contains("<b>tare</b>"), "{h}");
                assert!(h.contains("Bună ziua ș"), "{h}");
            }
            Body::Text(_) => panic!("html expected"),
        }
    }

    #[test]
    fn plain_rtf() {
        let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\f0 First line\par Second \'e9t\'e9\tab end}";
        match extract(rtf) {
            Body::Text(t) => assert_eq!(t, "First line\r\nSecond été\tend"),
            Body::Html(_) => panic!("text expected"),
        }
    }
}
