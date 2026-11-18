//! Mail: Outlook archives (PST, OST), Outlook messages (MSG), EML and MBOX into PDF,
//! EML, MBOX or HTML, attachments kept. PST through Microsoft's own reader
//! (outlook-pst), MSG with cfb, EML and MBOX with mail-parser. PDFs are printed by Edge
//! with scripts off and the internet blocked, so a mail can't tell its sender it was opened.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::words::Words;
use crate::common::{edge, rtf};
use contract::{opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MAIL: &[&str] = &["pst", "ost", "msg", "eml", "mbox"];

#[derive(Default, Clone)]
pub struct Att {
    name: String,
    mime: String,
    cid: Option<String>,
    data: Vec<u8>,
}

#[derive(Default, Clone)]
pub struct Mail {
    subject: String,
    from: Option<(String, String)>,
    to: Vec<(String, String)>,
    cc: Vec<(String, String)>,
    bcc: Vec<(String, String)>,
    /// Seconds since 1970, UTC.
    date: Option<i64>,
    message_id: String,
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<Att>,
    /// The message as it came (EML, MBOX): written back as it is.
    raw: Option<Vec<u8>>,
}

pub struct MailTool;

impl Plugin for MailTool {
    fn id(&self) -> &'static str {
        "mail"
    }

    fn manifest(&self) -> Value {
        let container = json!({ "file": "container", "eq": true, "quant": "any" });
        json!({
            "name": t("E-mailuri", "E-mail"),
            "summary": t("PST, MSG, EML, MBOX în PDF", "PST, MSG, EML, MBOX to PDF"),
            "keywords": { "ro": ["pst", "msg", "eml", "mbox", "outlook", "e-mail", "arhivă outlook"],
                          "en": ["pst", "msg", "eml", "mbox", "outlook", "email", "outlook archive"] },
            "category": "document",
            "icon": "mail",
            "input": { "extensions": MAIL, "multiple": true, "folders": true, "mode": "combined" },
            "output": { "kind": "perInput", "extensions": ["pdf", "eml", "mbox", "html", "folder"],
                        "nameVariables": [ { "id": "file", "label": t("numele", "name") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["container", "kind"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 2.0 },
            "presets": [],
            "groups": [],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "pdf", "display": "chips",
                  "options": [ opt("pdf", "PDF", "PDF"), opt("eml", "EML", "EML"), opt("mbox", "MBOX (Thunderbird…)", "MBOX (Thunderbird…)"), opt("html", "HTML", "HTML") ] },
                { "key": "onePdf", "type": "toggle", "default": false, "label": t("Toate mesajele unei arhive într-un singur PDF", "All the messages of an archive in one PDF"),
                  "showIf": { "all": [ { "field": "format", "eq": "pdf" }, container ] } },
                { "key": "attachments", "type": "toggle", "default": true, "label": t("Salvează și atașamentele", "Save the attachments too"),
                  "showIf": { "field": "format", "in": ["pdf", "html"] },
                  "help": t("În EML și MBOX atașamentele rămân în mesaje.", "In EML and MBOX the attachments stay inside the messages.") }
            ],
            "rules": [
                { "id": "folders", "severity": "info", "when": { "all": [ container, { "field": "format", "in": ["pdf", "eml", "html"] }, { "field": "onePdf", "ne": true } ] },
                  "message": t("O arhivă iese ca un dosar cu un fișier pentru fiecare mesaj, cu subdosarele ei.", "An archive comes out as a folder with a file for each message, with its subfolders.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{file}", "{file}") } },
            "errors": {
                "not_mail": t("Fișierul nu este un e-mail sau o arhivă pe care o putem citi.", "The file isn't an e-mail or an archive we can read."),
                "ost_unsupported": t("Acest OST e în formatul nou al Outlook, pe care nu-l putem citi. Exportă căsuța în PST din Outlook (Fișier › Deschidere și export).",
                                     "This OST is in Outlook's newer format, which we can't read. Export the mailbox to PST in Outlook (File › Open & Export)."),
                "same_format": t("Fișierul e deja în acest format.", "The file is already in this format."),
                "empty": t("Arhiva nu are niciun mesaj.", "The archive has no messages.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let ext = ext_of(f.input_path());
        let mut p = Map::new();
        p.insert("kind".into(), json!(ext));
        p.insert("container".into(), json!(matches!(ext.as_str(), "pst" | "ost" | "mbox")));
        Analysis::ok(p)
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        let format = ctx.req.str_or("format", "pdf");
        let (loose, containers): (Vec<&FileRef>, Vec<&FileRef>) = ctx.req.files.iter().partition(|f| matches!(ext_of(f.input_path()).as_str(), "eml" | "msg"));
        if format == "mbox" && !loose.is_empty() {
            ctx.run_one("*", || loose_to_mbox(ctx, &loose));
        } else {
            for f in &loose {
                if ctx.cancel.stop_after_current() {
                    break;
                }
                ctx.run_one(&f.id, || one_message(ctx, f));
            }
        }
        for f in containers {
            if ctx.cancel.stop_after_current() {
                break;
            }
            ctx.run_one(&f.id, || archive(ctx, f));
        }
        Ok(())
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn words(req: &Request) -> &'static Words {
    crate::common::words::of(req.locale.as_deref())
}

// ------------------------------------------------------------------ reading: EML / MBOX

fn from_eml(raw: Vec<u8>) -> Option<Mail> {
    let m = mail_parser::MessageParser::default().parse(&raw)?;
    let pair = |mb: mail_parser::Mailbox| (mb.name().unwrap_or("").to_string(), mb.address().unwrap_or("").to_string());
    let list = |l: Option<mail_parser::AddressList>| l.map(|l| l.mailboxes().map(pair).collect()).unwrap_or_default();
    let mut mail = Mail {
        subject: m.subject().unwrap_or("").to_string(),
        from: m.from().and_then(|l| l.first()).map(pair),
        to: list(m.to()),
        cc: list(m.cc()),
        bcc: list(m.bcc()),
        date: m.date().map(|d| d.to_timestamp()),
        message_id: m.message_id().unwrap_or("").to_string(),
        text: m.body_text(0).map(|c| c.into_owned()),
        html: m.body_html(0).map(|c| c.into_owned()),
        ..Default::default()
    };
    for p in m.attachments() {
        let mime = p.content_type().map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream"))).unwrap_or_else(|| "application/octet-stream".into());
        let name = match (p.attachment_name(), p.nested()) {
            (Some(n), _) => n.to_string(),
            (None, Some(n)) => format!("{}.eml", n.subject().unwrap_or("message")),
            _ => "attachment".into(),
        };
        mail.attachments.push(Att { name, mime, cid: p.content_id().map(|c| c.trim_matches(['<', '>']).to_string()), data: p.decoded().into_owned() });
    }
    mail.raw = Some(raw);
    Some(mail)
}

/// The messages of an MBOX, one at a time (big files are never read whole).
fn each_in_mbox(path: &Path, mut each: impl FnMut(Vec<u8>, f64) -> Result<(), FileError>) -> Result<usize, FileError> {
    let total = file_size(path).max(1) as f64;
    let mut r = std::io::BufReader::new(std::fs::File::open(path).map_err(super::io_err)?);
    let (mut cur, mut line, mut read, mut count) = (Vec::new(), Vec::new(), 0u64, 0usize);
    let mut started = false;
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line).map_err(super::io_err)?;
        read += n as u64;
        if n == 0 || line.starts_with(b"From ") {
            if started && !cur.is_empty() {
                // The blank line before the next "From " belongs to the separator.
                while cur.ends_with(b"\n") || cur.ends_with(b"\r") {
                    cur.pop();
                }
                cur.extend_from_slice(b"\r\n");
                count += 1;
                each(std::mem::take(&mut cur), read as f64 / total)?;
            }
            if n == 0 {
                break;
            }
            started = true;
            continue;
        }
        if !started {
            continue;
        }
        // mboxrd: ">From " lines had one ">" added.
        let unquoted = line.iter().position(|&b| b != b'>').filter(|&i| i > 0 && line[i..].starts_with(b"From ")).map(|_| &line[1..]).unwrap_or(&line);
        cur.extend_from_slice(unquoted);
    }
    Ok(count)
}

// ------------------------------------------------------------------ reading: MAPI (MSG, PST)

/// Properties of an Outlook item, wherever they come from.
trait Props {
    fn string(&self, id: u16) -> Option<String>;
    fn binary(&self, id: u16) -> Option<Vec<u8>>;
    /// Integers and times (FILETIME) of the fixed-size properties.
    fn number(&self, id: u16) -> Option<i64>;
}

const PR_SUBJECT: u16 = 0x0037;
const PR_CLIENT_SUBMIT_TIME: u16 = 0x0039;
const PR_MESSAGE_DELIVERY_TIME: u16 = 0x0E06;
const PR_SENDER_NAME: u16 = 0x0C1A;
const PR_SENDER_EMAIL: u16 = 0x0C1F;
const PR_SENDER_SMTP: u16 = 0x5D01;
const PR_SENT_REPR_NAME: u16 = 0x0042;
const PR_SENT_REPR_EMAIL: u16 = 0x0065;
const PR_SENT_REPR_SMTP: u16 = 0x5D02;
const PR_MESSAGE_ID: u16 = 0x1035;
const PR_BODY: u16 = 0x1000;
const PR_HTML: u16 = 0x1013;
const PR_RTF: u16 = 0x1009;
const PR_INTERNET_CPID: u16 = 0x3FDE;
const PR_DISPLAY_NAME: u16 = 0x3001;
const PR_EMAIL_ADDRESS: u16 = 0x3003;
const PR_SMTP_ADDRESS: u16 = 0x39FE;
const PR_RECIPIENT_TYPE: u16 = 0x0C15;
const PR_ATTACH_LONG_FILENAME: u16 = 0x3707;
const PR_ATTACH_FILENAME: u16 = 0x3704;
const PR_ATTACH_MIME: u16 = 0x370E;
const PR_ATTACH_CID: u16 = 0x3712;
const PR_ATTACH_DATA: u16 = 0x3701;

fn filetime_to_unix(ft: i64) -> i64 {
    ft / 10_000_000 - 11_644_473_600
}

/// A MAPI subject may start with "\x01\x0N": the length of a prefix like "RE: ".
fn clean_subject(s: String) -> String {
    let mut c = s.chars();
    if c.next() == Some('\u{1}') {
        c.next();
        return c.collect();
    }
    s
}

fn smtp(addr: Option<String>) -> Option<String> {
    addr.filter(|a| a.contains('@'))
}

fn mapi_mail(p: &dyn Props, recipients: Vec<(i64, String, String)>, attachments: Vec<Att>) -> Mail {
    let from_name = p.string(PR_SENT_REPR_NAME).or_else(|| p.string(PR_SENDER_NAME)).unwrap_or_default();
    let from_addr = smtp(p.string(PR_SENT_REPR_SMTP)).or_else(|| smtp(p.string(PR_SENDER_SMTP))).or_else(|| smtp(p.string(PR_SENT_REPR_EMAIL))).or_else(|| smtp(p.string(PR_SENDER_EMAIL))).unwrap_or_default();
    let html = p.binary(PR_HTML).map(|b| {
        let cp = p.number(PR_INTERNET_CPID).unwrap_or(65001) as u16;
        decode_codepage(&b, cp)
    }).or_else(|| p.string(PR_HTML));
    let text = p.string(PR_BODY);
    let (mut html, mut text) = (html.filter(|h| !h.trim().is_empty()), text.filter(|t| !t.trim().is_empty()));
    if html.is_none() {
        if let Some(r) = p.binary(PR_RTF).and_then(|c| unpack_rtf(&c)) {
            {
                match rtf::extract(&r) {
                    rtf::Body::Html(h) => html = Some(h),
                    rtf::Body::Text(t) if text.is_none() => text = Some(t),
                    _ => {}
                }
            }
        }
    }
    let mut m = Mail {
        subject: clean_subject(p.string(PR_SUBJECT).unwrap_or_default()),
        from: (!from_name.is_empty() || !from_addr.is_empty()).then_some((from_name, from_addr)),
        date: p.number(PR_CLIENT_SUBMIT_TIME).or_else(|| p.number(PR_MESSAGE_DELIVERY_TIME)).map(filetime_to_unix),
        message_id: p.string(PR_MESSAGE_ID).unwrap_or_default().trim_matches(['<', '>']).to_string(),
        text,
        html,
        attachments,
        ..Default::default()
    };
    for (kind, name, addr) in recipients {
        let list = match kind {
            2 => &mut m.cc,
            3 => &mut m.bcc,
            _ => &mut m.to,
        };
        list.push((name, addr));
    }
    m
}

/// PR_RTF_COMPRESSED: "LZFu" (compressed) or "MELA" (stored as it is).
fn unpack_rtf(c: &[u8]) -> Option<Vec<u8>> {
    let raw_size = u32::from_le_bytes(c.get(4..8)?.try_into().ok()?) as usize;
    if c.get(8..12)? == b"MELA" {
        return Some(c.get(16..)?.iter().take(raw_size).copied().collect());
    }
    // The decompressor panics on some damaged bodies: then there is no RTF body.
    std::panic::catch_unwind(|| compressed_rtf::decompress_rtf(c).ok()).ok().flatten().map(String::into_bytes)
}

fn decode_codepage(b: &[u8], cp: u16) -> String {
    let label = match cp {
        65001 | 0 => "utf-8".to_string(),
        20127 => "us-ascii".into(),
        28591..=28599 => format!("iso-8859-{}", cp - 28590),
        932 => "shift_jis".into(),
        936 => "gbk".into(),
        949 => "euc-kr".into(),
        950 => "big5".into(),
        n => format!("windows-{n}"),
    };
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    enc.decode(b).0.trim_end_matches('\0').to_string()
}

// --- MSG (a compound file: "__substg1.0_IIIITTTT" streams, fixed values in "__properties_version1.0")

struct MsgProps<'a> {
    file: std::cell::RefCell<&'a mut cfb::CompoundFile<std::fs::File>>,
    base: String,
    fixed: std::collections::HashMap<u16, i64>,
    codepage: u16,
}

/// The code page the .msg file itself names (0x3FFD, else the internet code page 0x3FDE).
fn root_codepage(file: &mut cfb::CompoundFile<std::fs::File>) -> Option<u16> {
    let mut v = Vec::new();
    file.open_stream("/__properties_version1.0").ok()?.read_to_end(&mut v).ok()?;
    let mut found = None;
    for e in v.get(32..).unwrap_or(&[]).as_chunks::<16>().0 {
        let tag = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
        let value = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u16;
        match tag {
            0x3FFD_0003 => return Some(value).filter(|c| *c != 0),
            0x3FDE_0003 => found = Some(value).filter(|c| *c != 0),
            _ => {}
        }
    }
    found
}

impl<'a> MsgProps<'a> {
    fn new(file: &'a mut cfb::CompoundFile<std::fs::File>, base: &str, header: usize) -> MsgProps<'a> {
        let mut fixed = std::collections::HashMap::new();
        if let Ok(mut s) = file.open_stream(format!("{base}/__properties_version1.0")) {
            let mut v = Vec::new();
            let _ = s.read_to_end(&mut v);
            for e in v.get(header..).unwrap_or(&[]).as_chunks::<16>().0 {
                let tag = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                let value = i64::from_le_bytes([e[8], e[9], e[10], e[11], e[12], e[13], e[14], e[15]]);
                let kind = tag & 0xFFFF;
                let value = if kind == 0x0003 { value as i32 as i64 } else { value };
                fixed.insert((tag >> 16) as u16, value);
            }
        }
        // The message's code page, else its internet code page (a Russian Outlook writes
        // only that one: 1251), else the message's, for a recipient or an attachment.
        let own = |f: &std::collections::HashMap<u16, i64>| f.get(&0x3FFD).or_else(|| f.get(&0x3FDE)).map(|&c| c as u16).filter(|c| *c != 0);
        let codepage = own(&fixed).or_else(|| (!base.is_empty()).then(|| root_codepage(file)).flatten()).unwrap_or(1252);
        MsgProps { file: std::cell::RefCell::new(file), base: base.to_string(), fixed, codepage }
    }

    fn stream(&self, id: u16, kind: u16) -> Option<Vec<u8>> {
        let mut f = self.file.borrow_mut();
        let mut s = f.open_stream(format!("{}/__substg1.0_{id:04X}{kind:04X}", self.base)).ok()?;
        let mut v = Vec::new();
        s.read_to_end(&mut v).ok()?;
        Some(v)
    }
}

impl Props for MsgProps<'_> {
    fn string(&self, id: u16) -> Option<String> {
        if let Some(b) = self.stream(id, 0x001F) {
            let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            return Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string());
        }
        self.stream(id, 0x001E).map(|b| decode_codepage(&b, self.codepage))
    }
    fn binary(&self, id: u16) -> Option<Vec<u8>> {
        self.stream(id, 0x0102)
    }
    fn number(&self, id: u16) -> Option<i64> {
        self.fixed.get(&id).copied()
    }
}

fn from_msg(path: &Path) -> Result<Mail, FileError> {
    let mut file = cfb::open(path).map_err(|_| FileError::new("not_mail"))?;
    Ok(msg_at(&mut file, "", 32, 0))
}

/// The message stored at `base` ("" for the file itself); `header` is the size of its
/// property stream's header (32 for the file, 24 for a message attached inside it).
fn msg_at(file: &mut cfb::CompoundFile<std::fs::File>, base: &str, header: usize, depth: u32) -> Mail {
    let names: Vec<String> = match file.read_storage(if base.is_empty() { "/" } else { base }) {
        Ok(entries) => entries.map(|e| e.name().to_string()).collect(),
        Err(_) => Vec::new(),
    };
    let mut recipients = Vec::new();
    for n in names.iter().filter(|n| n.starts_with("__recip_version1.0_")) {
        let p = MsgProps::new(file, &format!("{base}/{n}"), 8);
        let name = p.string(PR_DISPLAY_NAME).unwrap_or_default();
        let addr = smtp(p.string(PR_SMTP_ADDRESS)).or_else(|| smtp(p.string(PR_EMAIL_ADDRESS))).unwrap_or_default();
        recipients.push((p.number(PR_RECIPIENT_TYPE).unwrap_or(1), name, addr));
    }
    let mut attachments = Vec::new();
    for n in names.iter().filter(|n| n.starts_with("__attach_version1.0_")) {
        let at = format!("{base}/{n}");
        let p = MsgProps::new(file, &at, 8);
        let name = p.string(PR_ATTACH_LONG_FILENAME).or_else(|| p.string(PR_ATTACH_FILENAME)).or_else(|| p.string(PR_DISPLAY_NAME));
        match p.binary(PR_ATTACH_DATA) {
            Some(data) => attachments.push(Att {
                name: name.unwrap_or_else(|| "attachment".into()),
                mime: p.string(PR_ATTACH_MIME).unwrap_or_else(|| "application/octet-stream".into()),
                cid: p.string(PR_ATTACH_CID),
                data,
            }),
            // A message attached to the message (a forwarded mail): kept as an .eml that
            // any mail program opens.
            None if depth < 4 => {
                drop(p);
                let inner = format!("{at}/__substg1.0_3701000D");
                if file.is_storage(&inner) {
                    let m = msg_at(file, &inner, 24, depth + 1);
                    if let Ok(data) = eml_bytes(&m) {
                        // Its subject says more than Outlook's "Untitled Attachment".
                        let subject = Some(m.subject.trim().to_string()).filter(|t| !t.is_empty());
                        let title = subject.or(name.filter(|n| !n.trim().is_empty())).unwrap_or_else(|| "message".into());
                        let title = title.trim_end_matches(".msg").trim_end_matches(".eml").to_string();
                        attachments.push(Att { name: format!("{title}.eml"), mime: "message/rfc822".into(), cid: None, data });
                    }
                }
            }
            None => {}
        }
    }
    let p = MsgProps::new(file, base, header);
    mapi_mail(&p, recipients, attachments)
}

// --- PST / OST (Microsoft's reader)

mod pst {
    use super::{filetime_to_unix, mapi_mail, smtp, Att, Mail, Props, PR_ATTACH_CID, PR_ATTACH_FILENAME, PR_ATTACH_LONG_FILENAME, PR_ATTACH_MIME, PR_DISPLAY_NAME, PR_EMAIL_ADDRESS, PR_RECIPIENT_TYPE, PR_SMTP_ADDRESS};
    use outlook_pst::ltp::prop_context::PropertyValue;
    use outlook_pst::ltp::table_context::TableContext;
    use outlook_pst::messaging::attachment::{Attachment, AttachmentData};
    use outlook_pst::messaging::folder::Folder;
    use outlook_pst::messaging::message::Message;
    use outlook_pst::messaging::store::Store;
    use outlook_pst::ndb::node_id::NodeId;
    use std::rc::Rc;

    fn string(v: &PropertyValue) -> Option<String> {
        match v {
            PropertyValue::Unicode(s) => Some(String::from_utf16_lossy(s.buffer()).trim_end_matches('\0').to_string()),
            PropertyValue::String8(s) => Some(super::decode_codepage(s.buffer(), 1252)),
            _ => None,
        }
    }

    struct P<'a>(&'a dyn Fn(u16) -> Option<PropertyValue>);

    impl Props for P<'_> {
        fn string(&self, id: u16) -> Option<String> {
            (self.0)(id).as_ref().and_then(string)
        }
        fn binary(&self, id: u16) -> Option<Vec<u8>> {
            match (self.0)(id) {
                Some(PropertyValue::Binary(b)) => Some(b.buffer().to_vec()),
                _ => None,
            }
        }
        fn number(&self, id: u16) -> Option<i64> {
            match (self.0)(id) {
                Some(PropertyValue::Time(t)) => Some(t),
                Some(PropertyValue::Integer32(i)) => Some(i as i64),
                Some(PropertyValue::Integer64(i)) => Some(i),
                _ => None,
            }
        }
    }

    fn column(table: &dyn TableContext, row: &outlook_pst::ltp::table_context::TableRowData, id: u16) -> Option<PropertyValue> {
        let ctx = table.context();
        let cols = row.columns(ctx).ok()?;
        ctx.columns().iter().zip(cols).find(|(c, _)| c.prop_id() == id).and_then(|(c, v)| table.read_column(v.as_ref()?, c.prop_type()).ok())
    }

    fn recipients(message: &dyn Message) -> Vec<(i64, String, String)> {
        let Some(table) = message.recipient_table() else { return Vec::new() };
        table.rows_matrix().map(|row| {
            let get = |id| column(table.as_ref(), row, id);
            let kind = match get(PR_RECIPIENT_TYPE) {
                Some(PropertyValue::Integer32(k)) => k as i64,
                _ => 1,
            };
            let name = get(PR_DISPLAY_NAME).as_ref().and_then(string).unwrap_or_default();
            let addr = smtp(get(PR_SMTP_ADDRESS).as_ref().and_then(string)).or_else(|| smtp(get(PR_EMAIL_ADDRESS).as_ref().and_then(string))).unwrap_or_default();
            (kind, name, addr)
        }).collect()
    }

    fn attachment(a: &dyn Attachment) -> Option<Att> {
        let props = a.properties();
        let get = |id| props.get(id).and_then(string);
        let name = get(PR_ATTACH_LONG_FILENAME).or_else(|| get(PR_ATTACH_FILENAME)).or_else(|| get(PR_DISPLAY_NAME)).unwrap_or_else(|| "attachment".into());
        let mime = get(PR_ATTACH_MIME).unwrap_or_else(|| "application/octet-stream".into());
        let cid = get(PR_ATTACH_CID);
        match a.data()? {
            AttachmentData::Binary(b) => Some(Att { name, mime, cid, data: b.buffer().to_vec() }),
            AttachmentData::Message(m) => {
                // A forwarded message: its text as an .eml inside this one.
                let inner = message_to_mail(m.as_ref(), Vec::new());
                let data = super::eml_bytes(&inner).ok()?;
                let title = if inner.subject.is_empty() { name } else { inner.subject.clone() };
                Some(Att { name: format!("{}.eml", super::safe(&title)), mime: "message/rfc822".into(), cid: None, data })
            }
        }
    }

    fn message_to_mail(m: &dyn Message, attachments: Vec<Att>) -> Mail {
        let props = m.properties();
        let get = |id: u16| props.get(id).cloned();
        mapi_mail(&P(&get), recipients(m), attachments)
    }

    /// Every message: (folder path, mail). `count` is called first with the total.
    pub fn each(path: &std::path::Path, count: &mut dyn FnMut(usize), each: &mut dyn FnMut(&[String], Mail) -> Result<(), contract::FileError>) -> Result<(), contract::FileError> {
        macro_rules! walk {
            ($file:ty, $store:ty, $folder:ty, $message:ty, $attachment:ty) => {{
                let pst = <$file>::open(path).map_err(|e| err(e))?;
                let store = <$store>::read(Rc::new(pst)).map_err(|e| err(e))?;
                let root = store.properties().ipm_sub_tree_entry_id().map_err(|e| err(e))?;
                let root = <$folder>::read(store.clone(), &root).map_err(|e| err(e))?;
                // Folders first (with how many messages each has), then the messages.
                let mut folders: Vec<(Vec<String>, Rc<$folder>)> = Vec::new();
                let mut queue = vec![(Vec::<String>::new(), root)];
                while let Some((names, folder)) = queue.pop() {
                    if let Some(h) = folder.hierarchy_table() {
                        for row in h.rows_matrix() {
                            let Ok(id) = store.properties().make_entry_id(NodeId::from(u32::from(row.id()))) else { continue };
                            let Ok(sub) = <$folder>::read(store.clone(), &id) else { continue };
                            let name = sub.properties().display_name().unwrap_or_else(|_| "Folder".into());
                            let mut path = names.clone();
                            path.push(name);
                            queue.push((path, sub));
                        }
                    }
                    folders.push((names, folder));
                }
                folders.sort_by(|a, b| a.0.cmp(&b.0));
                count(folders.iter().map(|(_, f)| f.contents_table().map(|t| t.rows_matrix().count()).unwrap_or(0)).sum());
                for (names, folder) in &folders {
                    let Some(contents) = folder.contents_table() else { continue };
                    for row in contents.rows_matrix() {
                        let Ok(id) = store.properties().make_entry_id(NodeId::from(u32::from(row.id()))) else { continue };
                        let Ok(message) = <$message>::read(store.clone(), &id, None) else { continue };
                        let mut atts = Vec::new();
                        if let Some(table) = message.attachment_table() {
                            for arow in table.rows_matrix() {
                                if let Ok(a) = <$attachment>::read(message.clone(), NodeId::from(u32::from(arow.id())), None) {
                                    if let Some(att) = attachment(a.as_ref()) {
                                        atts.push(att);
                                    }
                                }
                            }
                        }
                        let mail = message_to_mail(message.as_ref(), atts);
                        each(names, mail)?;
                    }
                }
                Ok(())
            }};
        }
        use outlook_pst::messaging::{attachment::{AnsiAttachment, UnicodeAttachment}, folder::{AnsiFolder, UnicodeFolder}, message::{AnsiMessage, UnicodeMessage}, store::{AnsiStore, UnicodeStore}};
        if outlook_pst::UnicodePstFile::open(path).is_ok() {
            walk!(outlook_pst::UnicodePstFile, UnicodeStore, UnicodeFolder, UnicodeMessage, UnicodeAttachment)
        } else if outlook_pst::AnsiPstFile::open(path).is_ok() {
            walk!(outlook_pst::AnsiPstFile, AnsiStore, AnsiFolder, AnsiMessage, AnsiAttachment)
        } else {
            // A file that says "!BDN" but neither format opens: Outlook 2013+'s OST.
            let mut head = [0u8; 12];
            let _ = std::io::Read::read_exact(&mut std::fs::File::open(path).map_err(crate::plugins::io_err)?, &mut head);
            Err(contract::FileError::new(if &head[..4] == b"!BDN" { "ost_unsupported" } else { "not_mail" }))
        }
    }

    fn err(e: std::io::Error) -> contract::FileError {
        contract::FileError::with("not_mail", serde_json::json!({ "message": e.to_string() }))
    }

    #[allow(dead_code)]
    fn _unused(_: i64) -> i64 {
        filetime_to_unix(0)
    }
}

// ------------------------------------------------------------------ writing

fn eml_bytes(m: &Mail) -> Result<Vec<u8>, FileError> {
    if let Some(raw) = &m.raw {
        return Ok(raw.clone());
    }
    use mail_builder::MessageBuilder;
    use mail_builder::headers::address::Address;
    fn addr(p: &(String, String)) -> Address<'_> {
        if p.0.is_empty() || p.0 == p.1 { p.1.as_str().into() } else { (p.0.as_str(), p.1.as_str()).into() }
    }
    fn list(v: &[(String, String)]) -> Address<'_> {
        Address::new_list(v.iter().map(addr).collect())
    }
    let mut b = MessageBuilder::new().subject(m.subject.as_str());
    if let Some(f) = &m.from {
        b = b.from(addr(f));
    }
    if !m.to.is_empty() {
        b = b.to(list(&m.to));
    }
    if !m.cc.is_empty() {
        b = b.cc(list(&m.cc));
    }
    if !m.bcc.is_empty() {
        b = b.bcc(list(&m.bcc));
    }
    if let Some(d) = m.date {
        b = b.date(d);
    }
    if !m.message_id.is_empty() {
        b = b.message_id(m.message_id.as_str());
    }
    if let Some(t) = &m.text {
        b = b.text_body(t.as_str());
    }
    if let Some(h) = &m.html {
        b = b.html_body(h.as_str());
    }
    if m.text.is_none() && m.html.is_none() {
        b = b.text_body("");
    }
    for a in &m.attachments {
        b = match &a.cid {
            Some(cid) if m.html.as_deref().is_some_and(|h| h.contains(cid.as_str())) => b.inline(a.mime.as_str(), cid.as_str(), a.data.as_slice()),
            _ => b.attachment(a.mime.as_str(), a.name.as_str(), a.data.as_slice()),
        };
    }
    b.write_to_vec().map_err(super::io_err)
}

/// mboxrd: a "From " line before each message, "From " lines inside get a ">".
fn mbox_entry(m: &Mail, out: &mut Vec<u8>) -> Result<(), FileError> {
    let eml = eml_bytes(m)?;
    let sender = m.from.as_ref().map(|f| f.1.as_str()).filter(|a| !a.is_empty()).unwrap_or("MAILER-DAEMON");
    let when = chrono::DateTime::from_timestamp(m.date.unwrap_or(0), 0).unwrap_or_default().format("%a %b %e %H:%M:%S %Y");
    out.extend_from_slice(format!("From {sender} {when}\n").as_bytes());
    for line in eml.split_inclusive(|&b| b == b'\n') {
        let body = line.iter().position(|&b| b != b'>').map(|i| &line[i..]).unwrap_or(line);
        if body.starts_with(b"From ") {
            out.push(b'>');
        }
        out.extend_from_slice(line);
    }
    if !out.ends_with(b"\n") {
        out.extend_from_slice(b"\n");
    }
    out.extend_from_slice(b"\n");
    Ok(())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn who(v: &[(String, String)]) -> String {
    v.iter().map(|(n, a)| match (n.is_empty(), a.is_empty()) {
        (false, false) if n != a => format!("{} &lt;{}&gt;", esc(n), esc(a)),
        (false, _) => esc(n),
        _ => esc(a),
    }).collect::<Vec<_>>().join(", ")
}

fn local_date(ts: i64, w: &Words) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0).single() {
        Some(d) => d.format(w.date_format).to_string(),
        None => String::new(),
    }
}

/// The message as one page: a header, then its HTML (inline pictures inside, scripts
/// out) or its text.
fn html_section(m: &Mail, w: &Words, attachment_links: Option<&str>) -> String {
    let (l_from, l_to, l_cc, l_date, l_att) = (w.from, w.to, w.cc, w.date, w.attachments);
    let mut rows = String::new();
    if let Some(f) = &m.from {
        rows.push_str(&format!("<tr><td class=k>{l_from}</td><td>{}</td></tr>", who(std::slice::from_ref(f))));
    }
    if !m.to.is_empty() {
        rows.push_str(&format!("<tr><td class=k>{l_to}</td><td>{}</td></tr>", who(&m.to)));
    }
    if !m.cc.is_empty() {
        rows.push_str(&format!("<tr><td class=k>{l_cc}</td><td>{}</td></tr>", who(&m.cc)));
    }
    if let Some(d) = m.date {
        rows.push_str(&format!("<tr><td class=k>{l_date}</td><td>{}</td></tr>", local_date(d, w)));
    }
    let listed: Vec<String> = m.attachments.iter().filter(|a| !is_inline(m, a)).map(|a| format!("{} ({})", esc(&a.name), size(a.data.len()))).collect();
    if !listed.is_empty() {
        let extra = attachment_links.map(|l| format!(" <span class=note>→ {}</span>", esc(l))).unwrap_or_default();
        rows.push_str(&format!("<tr><td class=k>{l_att}</td><td>{}{extra}</td></tr>", listed.join(", ")));
    }
    let body = match (&m.html, &m.text) {
        (Some(h), _) => clean_html(h, m),
        (None, Some(t)) => format!("<pre>{}</pre>", esc(t)),
        _ => String::new(),
    };
    format!("<section class=mail><div class=hdr><h1>{}</h1><table>{rows}</table></div><div class=body>{body}</div></section>", esc(&m.subject))
}

fn is_inline(m: &Mail, a: &Att) -> bool {
    a.cid.as_deref().is_some_and(|c| !c.is_empty() && m.html.as_deref().is_some_and(|h| h.contains(&format!("cid:{c}"))))
}

fn size(n: usize) -> String {
    if n >= 1_048_576 { format!("{:.1} MB", n as f64 / 1_048_576.0) } else { format!("{} KB", n.div_ceil(1024)) }
}

/// The mail's HTML without its document wrapper and scripts, pictures from `cid:` inline.
fn clean_html(h: &str, m: &Mail) -> String {
    let lower = h.to_ascii_lowercase();
    // Styles from the head, then what is inside <body>.
    let mut styles = String::new();
    let mut at = 0;
    while let Some(s) = lower[at..].find("<style").map(|i| at + i) {
        let Some(e) = lower[s..].find("</style>").map(|i| s + i + 8) else { break };
        styles.push_str(&h[s..e]);
        at = e;
    }
    let inner = match (lower.find("<body"), lower.rfind("</body>")) {
        (Some(b), Some(e)) if b < e => {
            let open_end = lower[b..].find('>').map(|i| b + i + 1).unwrap_or(b);
            &h[open_end..e]
        }
        _ => h,
    };
    let mut out = format!("{styles}{inner}");
    // No scripts (Edge runs with scripts off too).
    loop {
        let lower = out.to_ascii_lowercase();
        let Some(s) = lower.find("<script") else { break };
        let e = lower[s..].find("</script>").map(|i| s + i + 9).unwrap_or(out.len());
        out.replace_range(s..e, "");
    }
    for a in &m.attachments {
        if let Some(cid) = a.cid.as_deref().filter(|c| !c.is_empty()) {
            let needle = format!("cid:{cid}");
            if out.contains(&needle) {
                use base64_engine::encode;
                out = out.replace(&needle, &format!("data:{};base64,{}", a.mime, encode(&a.data)));
            }
        }
    }
    out
}

mod base64_engine {
    pub fn encode(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            s.push(T[(n >> 18) as usize & 63] as char);
            s.push(T[(n >> 12) as usize & 63] as char);
            s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        s
    }
}

fn page(sections: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:\">\
         <style>@page{{size:A4;margin:14mm}}\
         body{{font:13px 'Segoe UI',Arial,sans-serif;color:#111;margin:0}}\
         section.mail{{break-after:page}} section.mail:last-child{{break-after:auto}}\
         .hdr{{border-bottom:1px solid #bbb;margin:0 0 12px;padding:0 0 8px}} .hdr h1{{font-size:18px;margin:0 0 8px}}\
         .hdr table{{border-collapse:collapse}} .hdr td{{padding:1px 10px 1px 0;vertical-align:top}} .hdr td.k{{color:#666;white-space:nowrap}}\
         .note{{color:#666}} pre{{white-space:pre-wrap;font:13px Consolas,monospace}} img{{max-width:100%}}         .body *{{max-width:100%!important;box-sizing:border-box}} .body img{{height:auto!important}} .body{{overflow-wrap:anywhere}}</style></head><body>{sections}</body></html>"
    )
}

pub fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let mut t: String = t.trim().chars().take(90).collect();
    while t.ends_with('.') || t.ends_with(' ') {
        t.pop();
    }
    if t.is_empty() {
        return "message".into();
    }
    // Names Windows keeps for devices ("con", "nul.txt", "COM1.pdf"): such a file can't
    // be made, so the attachment would be lost.
    let base = t.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4 && (base.starts_with("COM") || base.starts_with("LPT")) && matches!(base.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        t.insert(0, '_');
    }
    t
}

/// "2026-03-31 14.05 Subject", unique in `used`.
fn stem(m: &Mail, used: &mut HashSet<String>) -> String {
    use chrono::TimeZone;
    let date = m.date.and_then(|d| chrono::Local.timestamp_opt(d, 0).single()).map(|d| d.format("%Y-%m-%d %H.%M ").to_string()).unwrap_or_default();
    let subject = if m.subject.trim().is_empty() { "(no subject)".to_string() } else { m.subject.clone() };
    let base = safe(&format!("{date}{subject}"));
    let mut name = base.clone();
    let mut n = 2;
    while !used.insert(name.to_lowercase()) {
        name = format!("{base} ({n})");
        n += 1;
    }
    name
}

/// Writes one message into `dir` as `format`; attachments into "<name> - attachments".
fn write_message(ctx: &Ctx, m: &Mail, dir: &Path, used: &mut HashSet<String>, format: &str, key: &str) -> Result<PathBuf, FileError> {
    std::fs::create_dir_all(dir).map_err(super::io_err)?;
    let req = ctx.req;
    let w = words(req);
    let name = stem(m, used);
    let save_atts = req.bool_or("attachments", true) && matches!(format, "pdf" | "html");
    let att_dir_name = format!("{name} - {}", w.attachments_folder);
    let mut saved_any = false;
    if save_atts {
        let mut names = HashSet::new();
        for a in m.attachments.iter().filter(|a| !is_inline(m, a)) {
            let d = dir.join(&att_dir_name);
            std::fs::create_dir_all(&d).map_err(super::io_err)?;
            let mut n = safe(&a.name);
            let mut i = 2;
            while !names.insert(n.to_lowercase()) {
                n = format!("{i} {}", safe(&a.name));
                i += 1;
            }
            write_file(&d.join(n), &a.data)?;
            saved_any = true;
        }
    }
    let link = saved_any.then_some(att_dir_name.as_str());
    let out = match format {
        "eml" => {
            let p = dir.join(format!("{name}.eml"));
            write_file(&p, &eml_bytes(m)?)?;
            p
        }
        "html" => {
            let p = dir.join(format!("{name}.html"));
            write_file(&p, page(&html_section(m, w, link)).as_bytes())?;
            p
        }
        _ => {
            let p = dir.join(format!("{name}.pdf"));
            let html = ctx.work_dir.join(format!("mail-{key}.html"));
            write_file(&html, page(&html_section(m, w, link)).as_bytes())?;
            edge::print_with(ctx, key, &html, &p, Duration::from_secs(120), true)?;
            let _ = std::fs::remove_file(&html);
            p
        }
    };
    Ok(out)
}

fn read_loose(path: &Path) -> Result<Mail, FileError> {
    match ext_of(path).as_str() {
        "msg" => from_msg(path),
        _ => from_eml(std::fs::read(path).map_err(super::io_err)?).ok_or_else(|| FileError::new("not_mail")),
    }
}

/// An EML or MSG on its own.
fn one_message(ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
    let path = f.input_path();
    let in_bytes = file_size(path);
    let format = ctx.req.str_or("format", "pdf");
    if format == "eml" && ext_of(path) == "eml" {
        return Err(FileError::skipped("same_format", json!({})));
    }
    ctx.progress(&f.id, 0.0, "read", 1, 2);
    let m = read_loose(path)?;
    ctx.progress(&f.id, 0.3, "write", 2, 2);
    let dir = ctx.file_dir(&f.id)?;
    let folders = |d: &Path| -> HashSet<PathBuf> { std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default() };
    let before = folders(&dir);
    let mut used = HashSet::new();
    let out = write_message(ctx, &m, &dir, &mut used, format, &f.id)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = out.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    // The file keeps the input's name; its attachments come along as a folder.
    ctx.emit_output(&f.id, &out, &ext, json!({ "file": stem }), in_bytes);
    // The attachments folder this message made (not one already there).
    if let Some(att) = folders(&dir).into_iter().find(|p| !before.contains(p)) {
        let label = att.file_name().unwrap_or_default().to_string_lossy().split(" - ").last().unwrap_or("attachments").to_string();
        ctx.emit_folder(&f.id, &att, json!({ "file": format!("{stem} - {label}") }), 0);
    }
    ctx.progress(&f.id, 1.0, "write", 2, 2);
    Ok(Value::Null)
}

/// Loose messages into one MBOX.
fn loose_to_mbox(ctx: &Ctx, files: &[&FileRef]) -> Result<Value, FileError> {
    let mut out = Vec::new();
    let mut in_bytes = 0;
    for (i, f) in files.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.progress("*", i as f64 / files.len() as f64, "write", 1, 1);
        in_bytes += file_size(f.input_path());
        let m = read_loose(f.input_path())?;
        mbox_entry(&m, &mut out)?;
    }
    let dest = ctx.file_dir("mbox")?.join("mail.mbox");
    write_file(&dest, &out)?;
    let name = if files.len() == 1 { files[0].input_path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() } else { words(ctx.req).messages.into() };
    ctx.progress("*", 1.0, "write", 1, 1);
    ctx.emit_output("*", &dest, "mbox", json!({ "file": name }), in_bytes);
    Ok(Value::Null)
}

/// A PST, OST or MBOX: a folder of messages (with the archive's folders), one PDF or one MBOX.
fn archive(ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
    let path = f.input_path();
    let in_bytes = file_size(path);
    let ext = ext_of(path);
    let format = ctx.req.str_or("format", "pdf");
    if format == "mbox" && ext == "mbox" {
        return Err(FileError::skipped("same_format", json!({})));
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let one_pdf = format == "pdf" && ctx.req.bool_or("onePdf", false);
    let root = ctx.file_dir(&f.id)?.join("out");
    let w = words(ctx.req);
    ctx.progress(&f.id, 0.0, "read", 1, 2);
    let total = std::cell::Cell::new(0usize);
    let mut done = 0usize;
    let mut used: std::collections::HashMap<Vec<String>, HashSet<String>> = Default::default();
    let mut mbox: std::collections::BTreeMap<Vec<String>, Vec<u8>> = Default::default();
    let mut sections = String::new();
    let mut handle = |folders: &[String], m: Mail, fraction: Option<f64>| -> Result<(), FileError> {
        ctx.check_cancel()?;
        done += 1;
        let frac = fraction.unwrap_or_else(|| done as f64 / total.get().max(1) as f64);
        ctx.progress(&f.id, 0.02 + 0.9 * frac, "write", 2, 2);
        match format {
            "mbox" => mbox_entry(&m, mbox.entry(folders.to_vec()).or_default())?,
            _ if one_pdf => sections.push_str(&html_section(&m, w, None)),
            _ => {
                let dir = folders.iter().fold(root.clone(), |d, n| d.join(safe(n)));
                let u = used.entry(folders.to_vec()).or_default();
                write_message(ctx, &m, &dir, u, format, &format!("{}-{done}", f.id))?;
            }
        }
        Ok(())
    };
    if ext == "mbox" {
        each_in_mbox(path, |raw, frac| match from_eml(raw) {
            Some(m) => handle(&[], m, Some(frac)),
            None => Ok(()),
        })?;
    } else {
        pst::each(path, &mut |n| total.set(n), &mut |folders, m| handle(folders, m, None))?;
    }
    if done == 0 {
        return Err(FileError::new("empty"));
    }
    ctx.progress(&f.id, 0.93, "write", 2, 2);
    if one_pdf {
        let html = ctx.work_dir.join(format!("mail-{}.html", f.id));
        write_file(&html, page(&sections).as_bytes())?;
        let pdf = ctx.file_dir(&f.id)?.join("all.pdf");
        edge::print_with(ctx, &f.id, &html, &pdf, Duration::from_secs(600), true)?;
        let _ = std::fs::remove_file(&html);
        ctx.emit_output(&f.id, &pdf, "pdf", json!({ "file": stem }), in_bytes);
    } else if format == "mbox" {
        // One MBOX per folder of the archive (as Thunderbird imports them).
        std::fs::create_dir_all(&root).map_err(super::io_err)?;
        if mbox.len() == 1 {
            let dest = ctx.file_dir(&f.id)?.join("mail.mbox");
            write_file(&dest, mbox.values().next().unwrap())?;
            ctx.emit_output(&f.id, &dest, "mbox", json!({ "file": stem }), in_bytes);
        } else {
            for (folders, data) in &mbox {
                let name = if folders.is_empty() { "Inbox".to_string() } else { folders.iter().map(|n| safe(n)).collect::<Vec<_>>().join(" - ") };
                write_file(&root.join(format!("{name}.mbox")), data)?;
            }
            ctx.emit_folder(&f.id, &root, json!({ "file": stem }), in_bytes);
        }
    } else {
        ctx.emit_folder(&f.id, &root, json!({ "file": stem }), in_bytes);
    }
    ctx.progress(&f.id, 1.0, "write", 2, 2);
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mbox_round_trip() {
        let m = Mail { subject: "Salut".into(), from: Some(("Ana".into(), "ana@example.com".into())), text: Some("From the start\nline\n>From quoted".into()), date: Some(1_700_000_000), ..Default::default() };
        let mut out = Vec::new();
        mbox_entry(&m, &mut out).unwrap();
        mbox_entry(&m, &mut out).unwrap();
        let p = std::env::temp_dir().join("convertor-mbox-test.mbox");
        std::fs::write(&p, &out).unwrap();
        let mut got = Vec::new();
        let n = each_in_mbox(&p, |raw, _| {
            got.push(from_eml(raw).unwrap());
            Ok(())
        }).unwrap();
        assert_eq!(n, 2);
        assert_eq!(got[0].subject, "Salut");
        // E-mail lines end in CRLF; the "From " lines come back as they were.
        assert!(got[1].text.as_deref().unwrap().replace("\r\n", "\n").contains("From the start\nline\n>From quoted"), "{:?}", got[1].text);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn names() {
        let mut used = HashSet::new();
        let m = Mail { subject: "Re: raport / martie?".into(), ..Default::default() };
        assert_eq!(stem(&m, &mut used), "Re_ raport _ martie_");
        assert_eq!(stem(&m, &mut used), "Re_ raport _ martie_ (2)");
        assert_eq!(base64_engine::encode(b"hello"), "aGVsbG8=");
    }
}

#[cfg(test)]
mod safe_tests {
    #[test]
    fn reserved_windows_names_are_renamed() {
        assert_eq!(super::safe("con"), "_con");
        assert_eq!(super::safe("NUL.txt"), "_NUL.txt");
        assert_eq!(super::safe("com1.pdf"), "_com1.pdf");
        assert_eq!(super::safe("Lpt9"), "_Lpt9");
        assert_eq!(super::safe("aux .doc"), "_aux .doc");
        assert_eq!(super::safe("console.txt"), "console.txt");
        assert_eq!(super::safe("com10.txt"), "com10.txt");
        assert_eq!(super::safe("report. ."), "report");
        assert_eq!(super::safe("a/b:c"), "a_b_c");
        assert_eq!(super::safe(" . "), "message");
    }
}
