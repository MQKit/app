//! PDF to Excel: the tables of a PDF into worksheets (or CSV). PyMuPDF finds the tables
//! (with ruled lines, or lined up text); here they are joined when they go on over the
//! next page, and amounts, percentages and dates become real numbers and dates.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use crate::common::{pdf, text};
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

const SCRIPT: &str = include_str!("pdf_to_excel_tables.py");

pub struct PdfToExcel;

impl Plugin for PdfToExcel {
    fn id(&self) -> &'static str {
        "pdf-to-excel"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("PDF în Excel", "PDF to Excel"),
            "summary": t("Tabelele în foi de calcul", "Tables into spreadsheets"),
            "keywords": { "ro": ["pdf în excel", "pdf în xlsx", "tabel din pdf", "extras de cont", "pdf în csv"],
                          "en": ["pdf to excel", "pdf to xlsx", "table from pdf", "bank statement", "pdf to csv"] },
            "category": "pdf",
            "icon": "table-export",
            "requires": [ { "engine": "pymupdf" } ],
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["xlsx", "csv"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["pageCount", "needsPassword", "hasText"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "heavy", "diskFactor": 0.5 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "sheets", "type": "select", "label": t("Foi", "Sheets"), "default": "table", "display": "chips",
                  "options": [ opt("table", "O foaie pentru fiecare tabel", "A sheet per table"), opt("page", "O foaie pentru fiecare pagină", "A sheet per page"),
                               opt("one", "Totul pe o foaie", "All on one sheet") ] },
                { "key": "join", "type": "toggle", "default": true, "label": t("Unește tabelele care continuă pe pagina următoare", "Join tables that go on over the next page"),
                  "showIf": { "field": "sheets", "ne": "page" } },
                { "key": "find", "type": "select", "label": t("Ce caută", "What to look for"), "default": "tables",
                  "options": [ opt("tables", "Tabele (cu sau fără linii)", "Tables (with or without lines)"),
                               opt("text", "Tot textul, pe rânduri și coloane", "All the text, in rows and columns") ],
                  "help": t("Dacă un tabel nu e găsit, încearcă „Tot textul”.", "If a table isn't found, try “All the text”.") },
                { "key": "format", "type": "select", "label": t("Format", "Format"), "default": "xlsx", "display": "chips",
                  "options": [ opt("xlsx", "Excel (XLSX)", "Excel (XLSX)"), opt("csv", "CSV", "CSV") ] },
                { "key": "numbers", "type": "toggle", "default": true, "advanced": true, "group": "more",
                  "label": t("Sumele și datele devin numere și date", "Amounts and dates become numbers and dates"),
                  "help": t("Ca să poți aduna și sorta în Excel. Numerele cu zerouri în față (coduri, conturi) rămân text.",
                            "So you can add up and sort in Excel. Numbers with leading zeros (codes, accounts) stay text."),
                  "showIf": { "field": "format", "eq": "xlsx" } },
                { "key": "pages", "type": "pages", "syntax": "set", "label": t("Pagini", "Pages"), "default": "all", "advanced": true, "group": "more" }
            ],
            "rules": [
                { "id": "scanned", "severity": "warning", "when": { "file": "hasText", "eq": false, "quant": "any" },
                  "message": t("Un PDF pare scanat (doar imagini, fără text). Fă-l întâi căutabil cu „Text din imagini (OCR)”, apoi adaugă-l aici.",
                               "A PDF looks scanned (pictures only, no text). Make it searchable first with “Text from pictures (OCR)”, then add it here.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "no_tables": t("Nu am găsit tabele. Încearcă „Tot textul, pe rânduri și coloane”.", "No tables found. Try “All the text, in rows and columns”."),
                "no_text": t("PDF-ul e scanat: nu are text. Fă-l întâi căutabil cu „Text din imagini (OCR)”.", "The PDF is scanned: it has no text. Make it searchable first with “Text from pictures (OCR)”.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, mut props, code) = pdf::analyze(f, req);
        if status == "ok" {
            if let Ok(doc) = pdf::open_file(f, req) {
                props.insert("hasText".into(), json!(has_text(&doc)));
            }
        }
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        let count = pdf::open_file(f, req).map(|d| d.get_pages().len()).unwrap_or(0);
        let wanted: Vec<usize> = if count > 0 {
            pages::parse_set(req.str_or("pages", "all"), count as u32)
                .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": bad, "pageCount": count })))?
                .into_iter().map(|p| p as usize - 1).collect()
        } else {
            vec![]
        };
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let found = find_tables(ctx, f, &wanted)?;
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.9, "write", 2, 2);
        let tables = arrange(found, req);
        if tables.is_empty() {
            let doc = pdf::open_file(f, req)?;
            return Err(FileError::new(if has_text(&doc) { "no_tables" } else { "no_text" }));
        }
        let ro = req.locale.as_deref().unwrap_or("en").starts_with("ro");
        let sheets = sheets(tables, req.str_or("sheets", "table"), ro);
        let dir = ctx.file_dir(&f.id)?;
        if req.str_or("format", "xlsx") == "csv" {
            for (i, s) in sheets.iter().enumerate() {
                let out = dir.join(format!("{i}.csv"));
                write_file(&out, &csv(s))?;
                ctx.emit_output(&f.id, &out, "csv", json!({}), if i == 0 { in_bytes } else { 0 });
            }
        } else {
            let out = dir.join("result.xlsx");
            write_file(&out, &xlsx(&sheets, req.bool_or("numbers", true))?)?;
            ctx.emit_output(&f.id, &out, "xlsx", json!({}), in_bytes);
        }
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        Ok(Value::Null)
    }
}

/// Does the PDF have any text (rather than scanned pictures only)? A look at the first
/// pages and the drawings they use.
fn has_text(doc: &lopdf::Document) -> bool {
    let shows = |d: &[u8]| d.windows(2).any(|w| w == b"Tj" || w == b"TJ");
    for id in doc.get_pages().values().take(10) {
        if shows(&doc.get_page_content(*id)) {
            return true;
        }
    }
    doc.objects.values().filter_map(|o| o.as_stream().ok())
        .filter(|s| s.dict.get(b"Subtype").and_then(|v| v.as_name()).map(|n| n == b"Form").unwrap_or(false))
        .take(200)
        .any(|s| s.decompressed_content().map(|c| shows(&c)).unwrap_or_else(|_| shows(&s.content)))
}

/// One table as found on a page.
struct Found {
    page: usize,
    rows: Vec<Vec<String>>,
    /// Was it the last table on its page / the first one on its page?
    last: bool,
    first: bool,
}

/// Runs PyMuPDF on the file (in the portable Python) and collects its tables.
fn find_tables(ctx: &Ctx, f: &FileRef, wanted: &[usize]) -> Result<Vec<Found>, FileError> {
    let req = ctx.req;
    let missing = |c: &str| FileError::with(codes::ENGINE_MISSING, json!({ "component": c }));
    let python = req.engines.get("python").map(|e| e.path.join("python.exe")).filter(|p| p.is_file()).ok_or_else(|| missing("python"))?;
    let site = req.engines.get("pymupdf").map(|e| e.path.join("site-packages")).filter(|p| p.is_dir()).ok_or_else(|| missing("pymupdf"))?;
    std::fs::create_dir_all(&ctx.work_dir).map_err(super::io_err)?;
    let script = ctx.work_dir.join("pdf_to_excel_tables.py");
    write_file(&script, SCRIPT.as_bytes())?;
    let mut child = Command::new(python)
        .args(["-I", "-X", "utf8"]).arg(&script)
        .env("CONVERTOR_PYPATH", &site).env("PYTHONUTF8", "1")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| FileError::engine(format!("python: {e}")))?;
    let ask = json!({
        "path": f.input_path(),
        "password": req.secrets.files.get(&f.id),
        "pages": wanted,
        "find": req.str_or("find", "tables"),
    });
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{ask}");
    }
    let stdout = child.stdout.take().ok_or_else(|| FileError::engine("python: no output"))?;
    let mut found = Vec::new();
    let mut error = None;
    for line in std::io::BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if ctx.cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            ctx.check_cancel()?;
        }
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        match m["ev"].as_str() {
            Some("page") => {
                let done = m["done"].as_f64().unwrap_or(0.0);
                let of = m["of"].as_f64().unwrap_or(1.0).max(1.0);
                ctx.progress(&f.id, 0.85 * done / of, "read", 1, 2);
            }
            Some("page_tables") => {
                let page = m["page"].as_u64().unwrap_or(0) as usize;
                let list = m["tables"].as_array().cloned().unwrap_or_default();
                let n = list.len();
                for (i, t) in list.into_iter().enumerate() {
                    let mut rows: Vec<Vec<String>> = t["rows"].as_array().into_iter().flatten()
                        .map(|r| r.as_array().into_iter().flatten().map(|c| c.as_str().unwrap_or("").to_string()).collect())
                        .collect();
                    // A header PyMuPDF found just above the lines of the table.
                    if let Some(h) = t["header"].as_array() {
                        let names: Vec<String> = h.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect();
                        let width = rows.first().map(Vec::len).unwrap_or(0);
                        if names.len() == width && names.iter().filter(|s| !s.is_empty()).count() >= 2.min(width) && rows.first() != Some(&names) {
                            rows.insert(0, names);
                        }
                    }
                    found.push(Found { page, rows, first: i == 0, last: i + 1 == n });
                }
            }
            Some("error") => error = Some(m["code"].as_str().unwrap_or(codes::ENGINE_FAILED).to_string()),
            _ => {}
        }
    }
    let status = child.wait().map_err(|e| FileError::engine(format!("python: {e}")))?;
    if let Some(code) = error {
        return Err(FileError::new(&code));
    }
    if !status.success() {
        return Err(FileError::engine(format!("python exited with {status}")));
    }
    Ok(found)
}

/// One table to write, with the page it starts on.
struct Table {
    page: usize,
    rows: Vec<Vec<String>>,
}

/// Tables in order; with `join`, one that goes on at the top of the next page (same
/// columns) is added to the one before it, without its repeated header.
fn arrange(found: Vec<Found>, req: &Request) -> Vec<Table> {
    let join = req.bool_or("join", true) && req.str_or("sheets", "table") != "page";
    let mut out: Vec<Table> = Vec::new();
    let mut prev_last = false;
    let mut prev_page = usize::MAX;
    for f in found {
        let width = |r: &Vec<Vec<String>>| r.first().map(Vec::len).unwrap_or(0);
        let goes_on = join && f.first && prev_last && f.page == prev_page.wrapping_add(1)
            && out.last().is_some_and(|t| width(&t.rows) == width(&f.rows));
        prev_last = f.last;
        prev_page = f.page;
        if goes_on {
            let t = out.last_mut().unwrap();
            let mut rows = f.rows;
            if !rows.is_empty() && t.rows.first() == rows.first() {
                rows.remove(0);
            }
            t.rows.extend(rows);
        } else {
            out.push(Table { page: f.page, rows: f.rows });
        }
    }
    out.retain(|t| !t.rows.is_empty());
    out
}

struct Sheet {
    name: String,
    /// Tables on the sheet, each with the row it starts at.
    blocks: Vec<(u32, Vec<Vec<String>>)>,
}

fn sheets(tables: Vec<Table>, how: &str, ro: bool) -> Vec<Sheet> {
    let (table_word, page_word) = if ro { ("Tabel", "Pagina") } else { ("Table", "Page") };
    let mut out: Vec<Sheet> = Vec::new();
    for (i, t) in tables.into_iter().enumerate() {
        let name = match how {
            "page" => format!("{page_word} {}", t.page + 1),
            "one" => if ro { "Tabele".into() } else { "Tables".into() },
            _ => format!("{table_word} {}", i + 1),
        };
        match out.last_mut() {
            Some(s) if s.name == name => {
                let start = s.blocks.last().map(|(r, rows)| r + rows.len() as u32 + 1).unwrap_or(0);
                s.blocks.push((start, t.rows));
            }
            _ => out.push(Sheet { name, blocks: vec![(0, t.rows)] }),
        }
    }
    out
}

// ---- values ----

enum Val {
    Text(String),
    Number(f64, NumFmt),
    Date(u16, u8, u8, String),
}

/// How a number was written, to show it the same way in Excel.
#[derive(Clone, Default, PartialEq, Debug)]
struct NumFmt {
    decimals: usize,
    grouped: bool,
    percent: bool,
    /// A currency before or after the number, already quoted for the format.
    unit: String,
    unit_first: bool,
    brackets: bool,
}

impl NumFmt {
    fn code(&self) -> String {
        let dec = if self.decimals > 0 { format!(".{}", "0".repeat(self.decimals)) } else { String::new() };
        let mut f = if self.percent { format!("0{dec}%") } else { format!("{}{dec}", if self.grouped { "#,##0" } else { "0" }) };
        if !self.unit.is_empty() {
            f = if self.unit_first { format!("{} {f}", self.unit) } else { format!("{f} {}", self.unit) };
        }
        if self.brackets {
            f = format!("{f};({f})");
        }
        f
    }
}

/// How a column writes its numbers: which is the decimal mark, and dates day first?
struct Reading {
    decimal: char,
    day_first: bool,
    /// The table itself showed the decimal mark (else it is the computer's).
    sure: bool,
}

/// Looks at the cells of one table to tell "1.234,56" from "1,234.56" and 03/04 from 04/03.
fn reading(rows: &[Vec<String>]) -> Reading {
    let (mut comma, mut dot) = (0, 0);
    let (mut day, mut month) = (0, 0);
    // Each number of a cell on its own ("7,389,208 3,027,153"), words and dot leaders left out.
    let tokens = rows.iter().flatten().flat_map(|c| c.split_whitespace()).filter(|t| t.chars().any(|ch| ch.is_ascii_digit()));
    for c in tokens {
        let s: String = c.chars().filter(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == ',' || *ch == '/').collect();
        // The same mark twice in a number groups thousands (1,234,567 / 1.234.567), so the
        // other one is the decimal mark. (31.03.2026 is a date: its parts aren't thousands.)
        let grouped_by = |mark: char| {
            let parts: Vec<&str> = s.split(mark).collect();
            parts.len() >= 3 && (1..=3).contains(&parts[0].len()) && parts[1..].iter().all(|p| p.len() == 3 && p.chars().all(|c| c.is_ascii_digit()))
        };
        if !s.contains(['.', '/']) && grouped_by(',') {
            dot += 1;
            continue;
        }
        if !s.contains([',', '/']) && grouped_by('.') {
            comma += 1;
            continue;
        }
        if s.contains('/') {
            let parts: Vec<u32> = s.split('/').filter_map(|p| p.parse().ok()).collect();
            if parts.len() == 3 {
                if parts[0] > 12 {
                    day += 1;
                }
                if parts[1] > 12 {
                    month += 1;
                }
            }
            continue;
        }
        match (s.rfind(','), s.rfind('.')) {
            (Some(a), Some(b)) => {
                if a > b {
                    comma += 1
                } else {
                    dot += 1
                }
            }
            (Some(a), None) if s.len() - a - 1 != 3 => comma += 1,
            (None, Some(b)) if s.len() - b - 1 != 3 && s.matches('.').count() == 1 => dot += 1,
            _ => {}
        }
    }
    let decimal = if comma > dot { ',' } else if dot > comma { '.' } else { text::decimal_separator() };
    let day_first = if day > month { true } else if month > day { false } else { text::day_first() };
    Reading { decimal, day_first, sure: comma != dot }
}

const CURRENCY: &[&str] = &["€", "$", "£", "¥", "₹", "₽", "₺", "₩", "₪", "₴", "zł", "kr", "lei", "Kč", "Ft", "лв", "RON", "EUR", "USD", "GBP", "CHF", "PLN", "HUF", "CZK", "BGN", "MDL", "SEK", "NOK", "DKK", "JPY", "CNY", "INR", "TRY", "UAH", "RUB"];

fn value(s: &str, r: &Reading) -> Val {
    let text = || Val::Text(s.to_string());
    let t = s.trim();
    if t.is_empty() {
        return text();
    }
    if let Some(d) = date(t, r) {
        return d;
    }
    // Sign, brackets and currency around the digits.
    let mut body = t.replace(['\u{a0}', '\u{202f}'], " ");
    let mut unit = String::new();
    let mut unit_first = false;
    for c in CURRENCY {
        if let Some(rest) = body.strip_prefix(c) {
            (unit, unit_first) = (format!("\"{c}\""), true);
            body = rest.trim().to_string();
            break;
        }
        if let Some(rest) = body.strip_suffix(c) {
            unit = format!("\"{c}\"");
            body = rest.trim().to_string();
            break;
        }
    }
    let mut negative = false;
    if body.starts_with('(') && body.ends_with(')') {
        negative = true;
        body = body[1..body.len() - 1].trim().to_string();
    }
    if let Some(rest) = body.strip_prefix(['-', '−', '–']) {
        negative = !negative;
        body = rest.trim().to_string();
    } else if let Some(rest) = body.strip_suffix('-') {
        negative = !negative;
        body = rest.trim().to_string();
    } else if let Some(rest) = body.strip_prefix('+') {
        body = rest.trim().to_string();
    }
    let percent = body.ends_with('%');
    if percent {
        body = body.trim_end_matches('%').trim().to_string();
    }
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == ' ' || c == '\'') || !body.starts_with(|c: char| c.is_ascii_digit()) {
        return text();
    }
    // Nothing in the table tells "456,965" from "456.965": one mark before three digits is
    // read as thousands (far more common than three decimals), never as 456.965.
    let mut thousands = false;
    if !r.sure {
        let marks: Vec<usize> = body.char_indices().filter(|(_, c)| *c == ',' || *c == '.').map(|(i, _)| i).collect();
        if let [i] = marks[..] {
            let (a, b) = (&body[..i], &body[i + 1..]);
            if b.len() == 3 && b.chars().all(|c| c.is_ascii_digit()) && (1..=3).contains(&a.len()) && a.chars().all(|c| c.is_ascii_digit()) && a != "0" {
                body = format!("{a}{b}");
                thousands = true;
            }
        }
    }
    let group = if r.decimal == ',' { '.' } else { ',' };
    let (int_part, frac) = match body.rsplit_once(r.decimal) {
        Some((a, b)) if !b.contains([group, ' ', '\'']) => (a.to_string(), b.to_string()),
        Some(_) => return text(),
        None => (body.clone(), String::new()),
    };
    if int_part.contains(r.decimal) {
        return text();
    }
    // Thousands groups must be groups of three, marked one way ("3,027,153 150,544" is
    // two numbers, not one).
    if [group, ' ', '\''].iter().filter(|m| int_part.contains(**m)).count() > 1 {
        return text();
    }
    let groups: Vec<&str> = int_part.split([group, ' ', '\'']).collect();
    if groups.len() > 1 && (groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3)) {
        return text();
    }
    let digits: String = groups.concat();
    // Codes, account numbers, phone numbers: leading zeros or too long for Excel.
    if (digits.len() > 1 && digits.starts_with('0') && frac.is_empty()) || digits.len() > 15 {
        return text();
    }
    let Ok(mut n) = format!("{digits}.{}", if frac.is_empty() { "0" } else { &frac }).parse::<f64>() else { return text() };
    if negative {
        n = -n;
    }
    if percent {
        // Rounded to the digits written: 5.9 / 100 is 0.059000000000000004 in binary.
        let places = (frac.len() + 2).min(15) as i32;
        n = ((n / 100.0) * 10f64.powi(places)).round() / 10f64.powi(places);
    }
    let fmt = NumFmt { decimals: frac.len().min(10), grouped: groups.len() > 1 || thousands, percent, unit, unit_first, brackets: t.starts_with('(') };
    Val::Number(n, fmt)
}

/// 31.12.2026, 2026-12-31, 31/12/2026 (or 12/31/2026), with 2- or 4-digit years.
fn date(t: &str, r: &Reading) -> Option<Val> {
    let sep = ['.', '-', '/'].into_iter().find(|c| t.contains(*c))?;
    let parts: Vec<&str> = t.split(sep).collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || p.len() > 4 || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let n: Vec<u32> = parts.iter().map(|p| p.parse().unwrap_or(0)).collect();
    let year = |y: u32, len: usize| if len == 2 { 2000 + y } else { y };
    let (y, m, d, pattern) = if parts[0].len() == 4 {
        (n[0], n[1], n[2], format!("yyyy{sep}mm{sep}dd"))
    } else if parts[2].len() == 4 || parts[2].len() == 2 {
        let yy = if parts[2].len() == 2 { "yy" } else { "yyyy" };
        let day_first = sep == '.' || r.day_first;
        let (d, m) = if day_first { (n[0], n[1]) } else { (n[1], n[0]) };
        let pattern = if day_first { format!("dd{sep}mm{sep}{yy}") } else { format!("mm{sep}dd{sep}{yy}") };
        (year(n[2], parts[2].len()), m, d, pattern)
    } else {
        return None;
    };
    // A dash between short numbers is more likely a range or a code than a date.
    if sep == '-' && parts[0].len() != 4 {
        return None;
    }
    if !(1900..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    ExcelDateTime::from_ymd(y as u16, m as u8, d as u8).ok()?;
    Some(Val::Date(y as u16, m as u8, d as u8, pattern))
}

/// The first row is a header when it has text and no numbers, and numbers come below it.
fn has_header(rows: &[Vec<String>], r: &Reading) -> bool {
    let numeric = |row: &Vec<String>| row.iter().any(|c| !matches!(value(c, r), Val::Text(_)));
    rows.len() >= 2 && rows[0].iter().any(|c| !c.trim().is_empty()) && !numeric(&rows[0]) && rows[1..].iter().any(numeric)
}

fn xlsx(sheets: &[Sheet], numbers: bool) -> Result<Vec<u8>, FileError> {
    let bad = |e: rust_xlsxwriter::XlsxError| FileError::engine(e);
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let mut used: Vec<String> = Vec::new();
    for s in sheets {
        let ws = wb.add_worksheet();
        let mut name: String = s.name.chars().map(|c| if "[]:*?/\\".contains(c) { '_' } else { c }).take(31).collect();
        if used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
            name = format!("{} ({})", name.chars().take(25).collect::<String>(), used.len() + 1);
        }
        used.push(name.clone());
        ws.set_name(&name).map_err(bad)?;
        let mut formats: std::collections::HashMap<String, Format> = std::collections::HashMap::new();
        for (start, rows) in &s.blocks {
            let r = reading(rows);
            let header = has_header(rows, &r);
            let vals: Vec<Vec<Val>> = rows.iter().enumerate().map(|(i, row)| row.iter().map(|c| {
                if !numbers || (header && i == 0) { Val::Text(c.clone()) } else { value(c, &r) }
            }).collect()).collect();
            // One look per column: thousands groups and decimals as the most detailed cell.
            let width = rows.iter().map(Vec::len).max().unwrap_or(0);
            let mut look: Vec<(bool, usize)> = vec![(false, 0); width];
            for row in &vals {
                for (x, v) in row.iter().enumerate() {
                    if let Val::Number(_, f) = v {
                        if !f.percent {
                            look[x].0 |= f.grouped;
                            look[x].1 = look[x].1.max(f.decimals);
                        }
                    }
                }
            }
            for (i, row) in vals.into_iter().enumerate() {
                let y = start + i as u32;
                for (x, v) in row.into_iter().enumerate() {
                    let col = x;
                    let x = x as u16;
                    if matches!(&v, Val::Text(t) if t.is_empty()) {
                        continue;
                    }
                    if header && i == 0 {
                        if let Val::Text(t) = &v {
                            ws.write_string_with_format(y, x, t, &bold).map_err(bad)?;
                        }
                        continue;
                    }
                    match v {
                        Val::Text(t) => {
                            ws.write_string(y, x, &t).map_err(bad)?;
                        }
                        Val::Number(n, mut nf) => {
                            if !nf.percent {
                                (nf.grouped, nf.decimals) = look[col];
                            }
                            let fmt = nf.code();
                            let f = formats.entry(fmt.clone()).or_insert_with(|| Format::new().set_num_format(fmt));
                            ws.write_number_with_format(y, x, n, f).map_err(bad)?;
                        }
                        Val::Date(yy, mm, dd, fmt) => {
                            let f = formats.entry(fmt.clone()).or_insert_with(|| Format::new().set_num_format(fmt));
                            let d = ExcelDateTime::from_ymd(yy, mm, dd).map_err(bad)?;
                            ws.write_datetime_with_format(y, x, &d, f).map_err(bad)?;
                        }
                    }
                }
            }
            // One table on the sheet: its header stays in view and can filter.
            if s.blocks.len() == 1 && header {
                let width = rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as u16;
                ws.set_freeze_panes(1, 0).map_err(bad)?;
                ws.autofilter(0, 0, rows.len() as u32 - 1, width - 1).map_err(bad)?;
            }
        }
        ws.autofit();
    }
    wb.save_to_buffer().map_err(bad)
}

/// Text as it is in the PDF, split on what Excel on this PC splits CSV on; UTF-8 with BOM.
fn csv(s: &Sheet) -> Vec<u8> {
    let sep = match text::list_separator() {
        ';' => ';',
        '\t' => '\t',
        _ => ',',
    };
    let mut out = String::from("\u{feff}");
    let mut row_at = 0u32;
    for (start, rows) in &s.blocks {
        while row_at < *start {
            out.push_str("\r\n");
            row_at += 1;
        }
        for row in rows {
            let line: Vec<String> = row.iter().map(|c| {
                if c.contains([sep, '"', '\n', '\r']) {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c.clone()
                }
            }).collect();
            out.push_str(&line.join(&sep.to_string()));
            out.push_str("\r\n");
            row_at += 1;
        }
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str, decimal: char) -> Option<(f64, String)> {
        match value(s, &Reading { decimal, day_first: true, sure: true }) {
            Val::Number(n, f) => Some((n, f.code())),
            _ => None,
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(num("1.234,56", ','), Some((1234.56, "#,##0.00".into())));
        assert_eq!(num("1,234.56", '.'), Some((1234.56, "#,##0.00".into())));
        assert_eq!(num("-45,00", ','), Some((-45.0, "0.00".into())));
        assert_eq!(num("(250,75)", ','), Some((-250.75, "0.00;(0.00)".into())));
        assert_eq!(num("15,5%", ','), Some((0.155, "0.0%".into())));
        assert_eq!(num("5.9%", '.'), Some((0.059, "0.0%".into())));
        assert_eq!(num("29.2%", '.'), Some((0.292, "0.0%".into())));
        assert_eq!(num("1 200,00 lei", ','), Some((1200.0, "#,##0.00 \"lei\"".into())));
        assert_eq!(num("€ 99.90", '.'), Some((99.9, "\"€\" 0.00".into())));
        assert_eq!(num("0040123456", '.'), None);
        assert_eq!(num("RO49AAAA1B31007593840000", '.'), None);
        assert_eq!(num("12,34,5", ','), None);
        assert_eq!(num("abc 12", '.'), None);
        assert_eq!(num("3,027,153 150,544", '.'), None);
        assert_eq!(num("1 234 567,5", ','), Some((1234567.5, "#,##0.0".into())));
    }

    #[test]
    fn dates() {
        let r = Reading { decimal: ',', day_first: true, sure: true };
        assert!(matches!(date("31.03.2026", &r), Some(Val::Date(2026, 3, 31, _))));
        assert!(matches!(date("2026-03-31", &r), Some(Val::Date(2026, 3, 31, _))));
        assert!(matches!(date("04/03/2026", &Reading { decimal: '.', day_first: false, sure: true }), Some(Val::Date(2026, 4, 3, _))));
        assert!(date("10-12-14", &r).is_none());
        assert!(date("1.2.3", &r).is_none());
        assert!(date("32.01.2026", &r).is_none());
    }

    #[test]
    fn decimal_mark_from_the_table() {
        let rows = vec![vec!["1.234,56".to_string(), "12,5".into()], vec!["7,00".into(), "3".into()]];
        assert_eq!(reading(&rows).decimal, ',');
        let rows = vec![vec!["1,234.56".to_string(), "12.5".into()]];
        assert_eq!(reading(&rows).decimal, '.');
        // Thousands marks used twice tell the decimal mark; leaders and dates don't count.
        let rows = vec![vec!["Total. . . . .".to_string(), "10,690,561 456,965".into()], vec!["31.03.2026".into(), "9,739".into()]];
        let r = reading(&rows);
        assert_eq!(r.decimal, '.');
        assert!(matches!(value("456,965", &r), Val::Number(n, _) if n == 456965.0));
        let rows = vec![vec!["1.234.567".to_string()]];
        assert_eq!(reading(&rows).decimal, ',');
        // No clue at all: one mark before three digits is thousands, not three decimals.
        let r = Reading { decimal: ',', day_first: true, sure: false };
        assert!(matches!(value("456,965", &r), Val::Number(n, _) if n == 456965.0));
        assert!(matches!(value("0,125", &r), Val::Number(n, _) if n == 0.125));
        assert!(matches!(value("12,5", &r), Val::Number(n, _) if n == 12.5));
    }
}
