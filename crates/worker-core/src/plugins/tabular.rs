//! Spreadsheet data: tables between Excel, OpenDocument, CSV, JSON and XML, with the
//! separator and encoding Excel expects and every letter intact.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::path::Path;

pub struct Tabular;

const INPUTS: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv", "txt", "json"];

#[derive(Clone)]
enum Cell {
    Empty,
    Text(String),
    Number(f64),
    Bool(bool),
    /// A date and/or time from a workbook: written as ISO text ("2026-03-31 14:30") in
    /// CSV, JSON and XML, and as a real date in Excel (it was the serial number 46112).
    Date(When),
}

#[derive(Clone, Copy)]
struct When {
    ymd: (u16, u8, u8),
    hms: (u8, u8, u8),
    kind: WhenKind,
}

#[derive(Clone, Copy, PartialEq)]
enum WhenKind {
    Date,
    DateTime,
    Time,
}

impl When {
    fn from_excel(d: &calamine::ExcelDateTime) -> Option<When> {
        if d.is_duration() {
            return None;
        }
        let v = d.as_f64();
        let (y, mo, da, h, mi, se, ms) = d.to_ymd_hms_milli();
        // Rounded to the second (Excel stores 14:30 as 0.604166666…).
        let mut hms = (h, mi, se);
        if ms >= 500 {
            let total = (h as u32 * 3600 + mi as u32 * 60 + se as u32 + 1).min(86399);
            hms = ((total / 3600) as u8, (total / 60 % 60) as u8, (total % 60) as u8);
        }
        let kind = if (0.0..1.0).contains(&v) {
            WhenKind::Time
        } else if hms == (0, 0, 0) {
            WhenKind::Date
        } else {
            WhenKind::DateTime
        };
        Some(When { ymd: (y, mo, da), hms, kind })
    }

    fn text(&self) -> String {
        let (y, m, d) = self.ymd;
        let (h, mi, s) = self.hms;
        let time = if s == 0 { format!("{h:02}:{mi:02}") } else { format!("{h:02}:{mi:02}:{s:02}") };
        match self.kind {
            WhenKind::Date => format!("{y:04}-{m:02}-{d:02}"),
            WhenKind::DateTime => format!("{y:04}-{m:02}-{d:02} {time}"),
            WhenKind::Time => time,
        }
    }
}

impl Cell {
    fn text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(s) => s.clone(),
            Cell::Number(n) => fmt_num(*n),
            Cell::Bool(b) => b.to_string(),
            Cell::Date(w) => w.text(),
        }
    }
}

struct Sheet {
    name: String,
    rows: Vec<Vec<Cell>>,
}

impl Plugin for Tabular {
    fn id(&self) -> &'static str {
        "tabular"
    }

    fn manifest(&self) -> Value {
        let csv = json!({ "field": "format", "eq": "csv" });
        json!({
            "name": t("Date din tabele", "Spreadsheet data"),
            "summary": t("Excel, CSV, JSON, XML", "Excel, CSV, JSON, XML"),
            "keywords": { "ro": ["excel", "csv", "json", "xml", "tabel", "foaie de calcul"], "en": ["excel", "csv", "json", "xml", "table", "spreadsheet"] },
            "category": "document",
            "icon": "table",
            "input": { "extensions": INPUTS, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "multiPerInput", "extensions": ["xlsx", "csv", "json", "xml"],
                        "nameVariables": [ { "id": "sheet", "label": t("foaia", "sheet") } ], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["sheetCount"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 2.0 },
            "presets": [
                { "id": "excel", "label": t("CSV pentru Excel", "CSV for Excel"), "default": true, "values": { "format": "csv", "separator": "semicolon", "encoding": "utf8bom" } },
                { "id": "xlsx", "label": t("În Excel (XLSX)", "To Excel (XLSX)"), "values": { "format": "xlsx" } },
                { "id": "json", "label": t("JSON pentru programe", "JSON for apps"), "values": { "format": "json", "header": true } }
            ],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "format", "type": "select", "label": t("Format nou", "New format"), "default": "csv", "display": "chips",
                  "options": [ opt("xlsx", "Excel (XLSX)", "Excel (XLSX)"), opt("csv", "CSV", "CSV"), opt("json", "JSON", "JSON"), opt("xml", "XML", "XML") ] },
                { "key": "separator", "type": "select", "label": t("Separator", "Separator"), "default": "auto", "showIf": csv,
                  "options": [ opt("auto", "Ca Excel pe acest PC", "Like Excel on this PC"), opt("semicolon", "Punct și virgulă ;", "Semicolon ;"),
                               opt("comma", "Virgulă ,", "Comma ,"), opt("tab", "Tab", "Tab") ],
                  "help": t("Excel deschide direct un CSV cu separatorul din setările regionale Windows.", "Excel opens a CSV directly when it uses the separator from the Windows regional settings.") },
                { "key": "encoding", "type": "select", "label": t("Codare", "Encoding"), "default": "utf8bom", "showIf": csv,
                  "options": [ opt("utf8bom", "UTF-8 pentru Excel (cu BOM)", "UTF-8 for Excel (with BOM)"), opt("utf8", "UTF-8", "UTF-8"),
                               opt("windows", "Codarea Windows a acestui PC (programe vechi)", "This PC's Windows encoding (old programs)") ] },
                { "key": "sheets", "type": "select", "label": t("Foi", "Sheets"), "default": "all", "display": "chips",
                  "options": [ opt("first", "Doar prima", "Only the first"), opt("all", "Toate", "All") ],
                  "showIf": { "file": "sheetCount", "gt": 1, "quant": "any" } },
                { "key": "header", "type": "toggle", "default": true, "label": t("Primul rând e capul de tabel", "The first row is the header"),
                  "showIf": { "field": "format", "in": ["json", "xml", "xlsx"] } }
            ],
            "rules": [],
            "outputBlock": { "defaults": { "nameTemplate": t("{file}", "{file}") } },
            "errors": { "empty_table": t("Nu am găsit niciun tabel în fișier.", "No table was found in the file.") }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let mut p = Map::new();
        let ext = ext_of(f.input_path());
        let count = if matches!(ext.as_str(), "xlsx" | "xlsm" | "xls" | "xlsb" | "ods") {
            match open_book(f.input_path(), req.secrets.files.get(&f.id).map(String::as_str)) {
                Ok(wb) => calamine::Reader::sheet_names(&wb).len(),
                Err(e) if e.code == codes::PASSWORD_REQUIRED || e.code == codes::WRONG_PASSWORD => {
                    p.insert("needsPassword".into(), json!(true));
                    return Analysis { status: "locked", props: p, warnings: vec![], code: Some(e.code) };
                }
                Err(e) if e.code == "password_old_format" => return Analysis::status("failed", "password_old_format"),
                Err(_) => return Analysis::status("corrupt", codes::CORRUPT_FILE),
            }
        } else {
            1
        };
        p.insert("sheetCount".into(), json!(count));
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let path = f.input_path();
        let in_bytes = file_size(path);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let mut sheets = read(path, req.secrets.files.get(&f.id).map(String::as_str))?;
        if req.str_or("sheets", "all") == "first" {
            sheets.truncate(1);
        }
        sheets.retain(|s| !s.rows.is_empty());
        if sheets.is_empty() {
            return Err(FileError::new("empty_table"));
        }
        ctx.progress(&f.id, 0.4, "write", 2, 2);
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let dir = ctx.file_dir(&f.id)?;
        let format = req.str_or("format", "csv");
        let header = req.bool_or("header", true);
        let emit = |file: String, ext: &str, data: Vec<u8>, sheet: &str, first: bool| -> Result<(), FileError> {
            let out = dir.join(format!("{}.{ext}", sanitize(&file)));
            write_file(&out, &data)?;
            ctx.emit_output(&f.id, &out, ext, json!({ "file": file, "sheet": sheet }), if first { in_bytes } else { 0 });
            Ok(())
        };
        if format == "xlsx" {
            emit(stem.clone(), "xlsx", write_xlsx(&sheets, header)?, "", true)?;
        } else {
            let many = sheets.len() > 1;
            for (i, s) in sheets.iter().enumerate() {
                let file = if many { format!("{stem}-{}", s.name) } else { stem.clone() };
                let data = match format {
                    "json" => write_json(s, header),
                    "xml" => write_xml(s, header),
                    _ => write_csv(s, req)?,
                };
                emit(file, format, data, &s.name, i == 0)?;
            }
        }
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        Ok(json!({ "sheets": sheets.len() }))
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn bad(e: impl std::fmt::Display) -> FileError {
    FileError::with(codes::CORRUPT_FILE, json!({ "message": e.to_string() }))
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// A length of time from a workbook (days as a fraction): 26:30:00.
fn duration(days: f64) -> String {
    let secs = (days * 86400.0).round() as i64;
    let sign = if secs < 0 { "-" } else { "" };
    let s = secs.abs();
    format!("{sign}{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

// ------------------------------------------------------------------ reading

fn read(path: &Path, password: Option<&str>) -> Result<Vec<Sheet>, FileError> {
    match ext_of(path).as_str() {
        "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => read_book(path, password),
        "json" => read_json(path),
        _ => read_csv(path),
    }
}

/// A workbook, a password-protected one opened (in memory) with its password.
fn open_book(path: &Path, password: Option<&str>) -> Result<calamine::Sheets<std::io::Cursor<Vec<u8>>>, FileError> {
    if crate::common::offcrypto::legacy_encrypted(path) {
        return Err(FileError::new("password_old_format"));
    }
    let data = if crate::common::offcrypto::is_encrypted(path) {
        let pw = password.ok_or_else(|| FileError::new(codes::PASSWORD_REQUIRED))?;
        match crate::common::offcrypto::unlock(path, pw) {
            Ok(Some(zip)) => zip,
            Ok(None) => return Err(FileError::new("password_old_format")),
            Err(code) => return Err(FileError::new(code)),
        }
    } else {
        std::fs::read(path).map_err(super::io_err)?
    };
    calamine::open_workbook_auto_from_rs(std::io::Cursor::new(data)).map_err(bad)
}

fn read_book(path: &Path, password: Option<&str>) -> Result<Vec<Sheet>, FileError> {
    use calamine::{Data, Reader};
    let mut wb = open_book(path, password)?;
    let mut out = Vec::new();
    for name in wb.sheet_names().to_vec() {
        let range = wb.worksheet_range(&name).map_err(bad)?;
        let rows = range
            .rows()
            .map(|r| {
                r.iter()
                    .map(|c| match c {
                        Data::Empty => Cell::Empty,
                        Data::String(s) => Cell::Text(s.clone()),
                        Data::Float(f) => Cell::Number(*f),
                        Data::Int(i) => Cell::Number(*i as f64),
                        Data::Bool(b) => Cell::Bool(*b),
                        Data::DateTime(d) => When::from_excel(d).map(Cell::Date).unwrap_or_else(|| Cell::Text(duration(d.as_f64()))),
                        Data::DateTimeIso(s) | Data::DurationIso(s) => Cell::Text(s.replace('T', " ")),
                        other => Cell::Text(other.to_string()),
                    })
                    .collect()
            })
            .collect();
        out.push(Sheet { name, rows });
    }
    Ok(out)
}

/// The separator that splits the first lines most evenly.
pub(crate) fn csv_delimiter(text: &str) -> u8 {
    let sample: Vec<&str> = text.lines().take(10).collect();
    b";,\t|"
        .iter()
        .copied()
        .max_by_key(|d| {
            let counts: Vec<usize> = sample.iter().map(|l| l.bytes().filter(|b| b == d).count()).collect();
            let first = counts.first().copied().unwrap_or(0);
            if first > 0 && counts.iter().all(|c| *c == first) { first * 100 } else { first }
        })
        .unwrap_or(b',')
}

fn read_csv(path: &Path) -> Result<Vec<Sheet>, FileError> {
    let raw = std::fs::read(path).map_err(super::io_err)?;
    let text = crate::common::text::decode(&raw);
    let delim = csv_delimiter(&text);
    let decimal_comma = delim == b';';
    let mut rd = csv::ReaderBuilder::new().delimiter(delim).has_headers(false).flexible(true).from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for rec in rd.records() {
        let rec = rec.map_err(bad)?;
        rows.push(rec.iter().map(|v| guess(v, decimal_comma)).collect());
    }
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Sheet1".into());
    Ok(vec![Sheet { name, rows }])
}

/// "12", "3.5" (or "3,5" in a semicolon file) → a number; everything else stays text.
fn guess(v: &str, decimal_comma: bool) -> Cell {
    let t = v.trim();
    if t.is_empty() {
        return Cell::Empty;
    }
    // Leading zeros (phone numbers, codes) stay text.
    if t.len() > 1 && t.starts_with('0') && !t.starts_with("0.") && !t.starts_with("0,") {
        return Cell::Text(v.to_string());
    }
    // More digits than a number keeps (card numbers, long IDs): 12345678901234567890
    // would become 12345678901234567000.
    if t.chars().filter(char::is_ascii_digit).count() > 15 {
        return Cell::Text(v.to_string());
    }
    let n = if decimal_comma && t.matches(',').count() == 1 && !t.contains('.') { t.replace(',', ".") } else { t.to_string() };
    match n.parse::<f64>() {
        Ok(x) if x.is_finite() => Cell::Number(x),
        _ => Cell::Text(v.to_string()),
    }
}

fn read_json(path: &Path) -> Result<Vec<Sheet>, FileError> {
    let raw = std::fs::read(path).map_err(super::io_err)?;
    let v: Value = serde_json::from_slice(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw)).map_err(bad)?;
    let items = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.values().find_map(|x| x.as_array().cloned()).unwrap_or_default(),
        _ => vec![],
    };
    let cell = |x: &Value| match x {
        Value::Null => Cell::Empty,
        Value::Bool(b) => Cell::Bool(*b),
        Value::Number(n) => Cell::Number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Cell::Text(s.clone()),
        other => Cell::Text(other.to_string()),
    };
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    if items.iter().all(Value::is_object) && !items.is_empty() {
        let mut keys: Vec<String> = Vec::new();
        for it in &items {
            for k in it.as_object().unwrap().keys() {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
        rows.push(keys.iter().map(|k| Cell::Text(k.clone())).collect());
        for it in &items {
            rows.push(keys.iter().map(|k| it.get(k).map(cell).unwrap_or(Cell::Empty)).collect());
        }
    } else {
        for it in &items {
            rows.push(match it {
                Value::Array(a) => a.iter().map(cell).collect(),
                other => vec![cell(other)],
            });
        }
    }
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Ok(vec![Sheet { name, rows }])
}

// ------------------------------------------------------------------ writing

fn write_xlsx(sheets: &[Sheet], header: bool) -> Result<Vec<u8>, FileError> {
    use rust_xlsxwriter::{Format, Workbook};
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let date_fmt = Format::new().set_num_format("yyyy-mm-dd");
    let datetime_fmt = Format::new().set_num_format("yyyy-mm-dd hh:mm");
    let time_fmt = Format::new().set_num_format("hh:mm");
    let mut used: Vec<String> = Vec::new();
    for s in sheets {
        let ws = wb.add_worksheet();
        // Excel: at most 31 characters, none of []:*?/\, unique.
        let mut name: String = s.name.chars().map(|c| if "[]:*?/\\".contains(c) { '_' } else { c }).take(31).collect();
        if name.trim().is_empty() || used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
            name = format!("Foaia {}", used.len() + 1);
        }
        used.push(name.clone());
        ws.set_name(&name).map_err(bad)?;
        for (r, row) in s.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let (r, c) = (r as u32, c as u16);
                let res = match cell {
                    Cell::Empty => Ok(&mut *ws),
                    Cell::Text(t) if header && r == 0 => ws.write_string_with_format(r, c, t, &bold),
                    Cell::Text(t) => ws.write_string(r, c, t),
                    Cell::Number(n) => ws.write_number(r, c, *n),
                    Cell::Bool(b) => ws.write_boolean(r, c, *b),
                    Cell::Date(w) => {
                        let (y, m, d) = w.ymd;
                        let (h, mi, se) = w.hms;
                        let (dt, fmt) = match w.kind {
                            WhenKind::Time => (rust_xlsxwriter::ExcelDateTime::from_hms(h as u16, mi, se), &time_fmt),
                            kind => (
                                rust_xlsxwriter::ExcelDateTime::from_ymd(y, m, d).and_then(|x| x.and_hms(h as u16, mi, se)),
                                if kind == WhenKind::Date { &date_fmt } else { &datetime_fmt },
                            ),
                        };
                        match dt {
                            Ok(dt) => ws.write_datetime_with_format(r, c, &dt, fmt),
                            Err(_) => ws.write_string(r, c, w.text()),
                        }
                    }
                };
                res.map_err(bad)?;
            }
        }
        if header && !s.rows.is_empty() {
            ws.set_freeze_panes(1, 0).map_err(bad)?;
        }
        ws.autofit();
        // Autofit doesn't measure dates: a date-and-time column showed "##########".
        let width = s.rows.iter().map(Vec::len).max().unwrap_or(0);
        for c in 0..width {
            if s.rows.iter().any(|r| matches!(r.get(c), Some(Cell::Date(_)))) {
                let chars = s.rows.iter().filter_map(|r| r.get(c)).map(|v| v.text().chars().count()).max().unwrap_or(10);
                ws.set_column_width(c as u16, chars as f64 + 2.0).map_err(bad)?;
            }
        }
    }
    wb.save_to_buffer().map_err(bad)
}

fn write_csv(s: &Sheet, req: &Request) -> Result<Vec<u8>, FileError> {
    let delim = match req.str_or("separator", "auto") {
        "comma" => b',',
        "tab" => b'\t',
        "semicolon" => b';',
        // What Excel on this PC splits on (the Windows list separator).
        _ => match crate::common::text::list_separator() {
            ';' => b';',
            '\t' => b'\t',
            _ => b',',
        },
    };
    // Semicolon files go with a decimal comma (a comma already separates the columns).
    let comma_decimal = delim == b';';
    let mut w = csv::WriterBuilder::new().delimiter(delim).terminator(csv::Terminator::CRLF).flexible(true).from_writer(Vec::new());
    for row in &s.rows {
        let mut fields: Vec<String> = row.iter().map(|c| match c {
            // A decimal comma where Windows uses one (so Excel reads it as a number).
            Cell::Number(n) if comma_decimal => fmt_num(*n).replace('.', ","),
            other => other.text(),
        }).collect();
        while fields.last().map(|f| f.is_empty()).unwrap_or(false) && fields.len() > 1 {
            fields.pop();
        }
        w.write_record(&fields).map_err(bad)?;
    }
    let text = String::from_utf8(w.into_inner().map_err(bad)?).unwrap_or_default();
    Ok(match req.str_or("encoding", "utf8bom") {
        "utf8" => text.into_bytes(),
        "windows" | "windows1250" => crate::common::text::windows_encoding().encode(&text).0.into_owned(),
        _ => {
            let mut b = vec![0xEF, 0xBB, 0xBF];
            b.extend_from_slice(text.as_bytes());
            b
        }
    })
}

fn cell_json(c: &Cell) -> Value {
    match c {
        Cell::Empty => Value::Null,
        Cell::Text(s) => json!(s),
        Cell::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => json!(*n as i64),
        Cell::Number(n) => json!(n),
        Cell::Bool(b) => json!(b),
        Cell::Date(w) => json!(w.text()),
    }
}

/// The header names, made unique and never empty.
fn keys(s: &Sheet) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let width = s.rows.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..width {
        let mut k = s.rows[0].get(i).map(Cell::text).unwrap_or_default().trim().to_string();
        if k.is_empty() {
            k = format!("col{}", i + 1);
        }
        let base = k.clone();
        let mut n = 2;
        while out.contains(&k) {
            k = format!("{base}_{n}");
            n += 1;
        }
        out.push(k);
    }
    out
}

fn write_json(s: &Sheet, header: bool) -> Vec<u8> {
    let v = if header && !s.rows.is_empty() {
        let keys = keys(s);
        let items: Vec<Value> = s.rows[1..]
            .iter()
            .map(|row| Value::Object(keys.iter().enumerate().map(|(i, k)| (k.clone(), row.get(i).map(cell_json).unwrap_or(Value::Null))).collect()))
            .collect();
        Value::Array(items)
    } else {
        Value::Array(s.rows.iter().map(|r| Value::Array(r.iter().map(cell_json).collect())).collect())
    };
    serde_json::to_vec_pretty(&v).unwrap_or_default()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn write_xml(s: &Sheet, header: bool) -> Vec<u8> {
    let mut x = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<table name=\"{}\">\n", xml_escape(&s.name));
    let (keys, rows) = if header && !s.rows.is_empty() { (keys(s), &s.rows[1..]) } else { (vec![], &s.rows[..]) };
    for row in rows {
        x.push_str("  <row>");
        for (i, c) in row.iter().enumerate() {
            let name = keys.get(i).cloned().unwrap_or_else(|| format!("col{}", i + 1));
            x.push_str(&format!("<cell name=\"{}\">{}</cell>", xml_escape(&name), xml_escape(&c.text())));
        }
        x.push_str("</row>\n");
    }
    x.push_str("</table>\n");
    x.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_ids_stay_text_and_durations_read() {
        assert!(matches!(guess("12345678901234567890", false), Cell::Text(_)));
        assert!(matches!(guess("123456789012345", false), Cell::Number(_)));
        assert!(matches!(guess("12,5", true), Cell::Number(n) if n == 12.5));
        assert_eq!(duration(1.1041666666666667), "26:30:00");
        let d = calamine::ExcelDateTime::new(46112.604166666664, calamine::ExcelDateTimeType::DateTime, false);
        assert_eq!(When::from_excel(&d).unwrap().text(), "2026-03-31 14:30");
    }
}
