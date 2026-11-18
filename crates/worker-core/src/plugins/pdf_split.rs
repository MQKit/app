//! Split PDF: one PDF into several, by pages, ranges, cut points, odd/even pages,
//! chapters (the bookmarks) or size. Each piece copies only the objects its pages use.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::pdf;
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use lopdf::{Document, ObjectId, SaveOptions};
use serde_json::{json, Value};

pub struct PdfSplit;

/// One output: 1-based pages, in order, and its name variables.
struct Piece {
    pages: Vec<u32>,
    chapter: String,
}

impl Plugin for PdfSplit {
    fn id(&self) -> &'static str {
        "pdf-split"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Împărțire PDF", "Split PDF"),
            "summary": t("Unul în mai multe", "One into many"),
            "keywords": { "ro": ["împarte", "taie", "separă", "extrage pagini", "capitole"], "en": ["split", "cut", "separate", "extract pages", "chapters"] },
            "category": "pdf",
            "icon": "scissors",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "multiPerInput", "extensions": ["pdf"],
                        "nameVariables": [ { "id": "from", "label": t("de la pagina", "from page") }, { "id": "to", "label": t("până la pagina", "to page") },
                                           { "id": "chapter", "label": t("capitol", "chapter") } ],
                        "replaceOriginalAllowed": false },
            "analysis": { "fast": ["needsPassword", "pageCount", "hasBookmarks", "hasForms", "isEncrypted"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.5 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "mode", "type": "select", "label": t("Cum împart", "How to split"), "default": "eachPage",
                  "options": [ opt("eachPage", "Fiecare pagină separat", "Every page on its own"),
                               opt("everyN", "La fiecare câteva pagini", "Every few pages"),
                               opt("ranges", "După intervale", "By ranges"),
                               opt("extract", "Extrage niște pagini într-un PDF", "Extract some pages into one PDF"),
                               opt("atPages", "Taie după anumite pagini", "Cut after certain pages"),
                               opt("byBookmarks", "Pe capitole (după cuprins)", "By chapter (bookmarks)"),
                               opt("bySize", "Bucăți sub o mărime", "Pieces under a size"),
                               opt("oddEven", "Pagini pare și impare", "Odd and even pages") ] },
                { "key": "pagesPerFile", "type": "number", "label": t("Pagini în fiecare fișier", "Pages in each file"), "default": 2,
                  "min": 1, "max": 9999, "showIf": { "field": "mode", "eq": "everyN" } },
                { "key": "ranges", "type": "text", "label": t("Intervale", "Ranges"), "default": "",
                  "placeholder": t("1-3; 4-10; 11-z", "1-3; 4-10; 11-z"),
                  "help": t("Fiecare interval devine un fișier. Separă-le cu ; (z = ultima pagină).", "Each range becomes a file. Separate them with ; (z = last page)."),
                  "showIf": { "field": "mode", "eq": "ranges" } },
                { "key": "pages", "type": "pages", "syntax": "ordered", "label": t("Pagini", "Pages"), "default": "all",
                  "placeholder": t("de exemplu 1-3, 7", "for example 1-3, 7"), "help": t("În ordinea scrisă; z = ultima pagină.", "In the order written; z = last page."),
                  "showIf": { "field": "mode", "eq": "extract" } },
                { "key": "splitPoints", "type": "text", "label": t("Taie după paginile", "Cut after pages"), "default": "",
                  "placeholder": t("de exemplu 3, 10, 25", "for example 3, 10, 25"),
                  "showIf": { "field": "mode", "eq": "atPages" } },
                { "key": "bookmarkLevel", "type": "select", "label": t("Nivel în cuprins", "Bookmark level"), "default": 1,
                  "options": [ opt(1, "Capitole (nivelul 1)", "Chapters (level 1)"), opt(2, "Subcapitole (nivelul 2)", "Sections (level 2)"),
                               opt(3, "Nivelul 3", "Level 3") ],
                  "showIf": { "field": "mode", "eq": "byBookmarks" } },
                { "key": "maxSize", "type": "number", "label": t("Mărime maximă", "Maximum size"), "default": 10000000,
                  "min": 50000, "max": 2000000000, "showIf": { "field": "mode", "eq": "bySize" },
                  "units": { "base": "B", "display": [ { "id": "MB", "factor": 1000000 }, { "id": "KB", "factor": 1000 } ], "defaultDisplay": "MB" },
                  "suggestions": [ { "value": 25000000, "label": t("25 MB (Gmail)", "25 MB (Gmail)") }, { "value": 10000000, "label": t("10 MB", "10 MB") },
                                   { "value": 5000000, "label": t("5 MB", "5 MB") }, { "value": 2000000, "label": t("2 MB", "2 MB") } ] },
                { "key": "oddEven", "type": "select", "label": t("Ce păstrez", "What to keep"), "default": "both", "display": "chips",
                  "options": [ opt("both", "Două fișiere", "Two files"), opt("odd", "Doar impare", "Odd only"), opt("even", "Doar pare", "Even only") ],
                  "showIf": { "field": "mode", "eq": "oddEven" } },
                { "key": "lastChunk", "type": "select", "advanced": true, "group": "more", "default": "keep",
                  "label": t("Ultima bucată, dacă e mai scurtă", "The last piece, if shorter"),
                  "options": [ opt("keep", "Rămâne separat", "Stays on its own"), opt("merge", "Se lipește de cea dinainte", "Joins the one before") ],
                  "showIf": { "field": "mode", "eq": "everyN" } },
                { "key": "keepInfo", "type": "toggle", "advanced": true, "group": "more", "default": true,
                  "label": t("Păstrează titlul și autorul", "Keep the title and author") }
            ],
            "rules": [
                // What would only fail once started is said before: the button waits for it.
                { "id": "type_ranges", "severity": "error", "fields": ["ranges"],
                  "when": { "all": [ { "field": "mode", "eq": "ranges" }, { "field": "ranges", "eq": "" } ] },
                  "message": t("Scrie intervalele, de exemplu 1-3; 4-10.", "Type the ranges, for example 1-3; 4-10.") },
                { "id": "type_split_points", "severity": "error", "fields": ["splitPoints"],
                  "when": { "all": [ { "field": "mode", "eq": "atPages" }, { "field": "splitPoints", "eq": "" } ] },
                  "message": t("Scrie după ce pagini se taie, de exemplu 3, 10.", "Type after which pages to cut, for example 3, 10.") },
                { "id": "no_bookmarks_any", "when": { "all": [ { "field": "mode", "eq": "byBookmarks" }, { "file": "hasBookmarks", "eq": false, "quant": "all" } ] },
                  "severity": "error",
                  "message": t("Nu e niciun cuprins după care să împart pe capitole.", "There are no bookmarks to split by chapter.") },
                { "id": "no_bookmarks", "when": { "all": [ { "field": "mode", "eq": "byBookmarks" }, { "file": "hasBookmarks", "eq": false },
                                                        { "not": { "file": "hasBookmarks", "eq": false, "quant": "all" } } ] },
                  "severity": "warning",
                  "message": t("Un fișier nu are cuprins; nu am după ce să-l împart pe capitole.", "A file has no bookmarks, so it can't be split by chapter.") },
                { "id": "password_dropped", "when": { "file": "isEncrypted", "eq": true }, "severity": "info",
                  "message": t("Bucățile păstrează parola fișierului.", "The pieces keep the file's password.") },
                { "id": "forms_lost", "when": { "file": "hasForms", "eq": true }, "severity": "warning",
                  "message": t("Câmpurile de formular nu vor mai putea fi completate în bucăți.", "Form fields won't be fillable in the pieces.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_{n}", "{name}_{n}"), "outputDir": "subfolder", "subfolderName": "{name}" } },
            "errors": {
                "no_bookmarks": t("Fișierul nu are cuprins la nivelul ales.", "The file has no bookmarks at the chosen level."),
                "empty_selection": t("Nu ai ales nicio pagină.", "No pages were chosen."),
                "piece_over_size": t("O pagină e mai mare decât limita singură; a rămas într-un fișier separat, puțin peste.",
                                     "A page is larger than the limit by itself; it's in a file of its own, a little over.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let in_bytes = file_size(f.input_path());
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let mut doc = pdf::open_file(f, ctx.req)?;
        // Bookmarks and links by name keep working in the new file(s).
        pdf::resolve_named_dests(&mut doc);
        let ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        let total = ids.len() as u32;
        for p in &ids {
            pdf::materialize_inherited(&mut doc, *p);
        }
        let pieces = plan(ctx, &doc, &ids, total)?;
        if pieces.is_empty() || pieces.iter().all(|p| p.pages.is_empty()) {
            return Err(FileError::new("empty_selection"));
        }
        let keep_info = ctx.req.bool_or("keepInfo", true);
        let dir = ctx.file_dir(&f.id)?;
        let width = pieces.len().to_string().len().max(2);
        let n = pieces.len();
        for (i, piece) in pieces.iter().enumerate() {
            ctx.check_cancel()?;
            ctx.progress(&f.id, 0.1 + 0.9 * i as f64 / n as f64, "write", 2, 2);
            let page_ids: Vec<ObjectId> = piece.pages.iter().map(|p| ids[(*p - 1) as usize]).collect();
            let mut part = pdf::extract_pages(&doc, &page_ids, keep_info);
            let path = dir.join(format!("part-{}.pdf", i + 1));
            pdf::save(&mut part, &path, false)?;
            if ctx.req.str_or("mode", "") == "bySize" && file_size(&path) > ctx.req.f64_or("maxSize", 10_000_000.0) as u64 {
                ctx.out.warning(Some(&f.id), "piece_over_size", Some(json!({ "n": i + 1 })));
            }
            let vars = json!({
                "n": format!("{:0width$}", i + 1, width = width),
                "from": piece.pages.first().copied().unwrap_or(0),
                "to": piece.pages.last().copied().unwrap_or(0),
                "chapter": piece.chapter,
                "pages": piece.pages.len(),
            });
            ctx.emit_output(&f.id, &path, "pdf", vars, if i == 0 { in_bytes } else { 0 });
        }
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        Ok(json!({ "pieces": n }))
    }
}

fn piece(pages: Vec<u32>) -> Piece {
    Piece { pages, chapter: String::new() }
}

fn bad_range(part: String, total: u32) -> FileError {
    FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "part": part, "pageCount": total }))
}

/// Which pages go into which file.
fn plan(ctx: &Ctx, doc: &Document, ids: &[ObjectId], total: u32) -> Result<Vec<Piece>, FileError> {
    let req = ctx.req;
    let all: Vec<u32> = (1..=total).collect();
    Ok(match req.str_or("mode", "eachPage") {
        "everyN" => {
            let n = (req.f64_or("pagesPerFile", 2.0).max(1.0) as usize).min(total.max(1) as usize);
            let mut out: Vec<Piece> = all.chunks(n).map(|c| piece(c.to_vec())).collect();
            if req.str_or("lastChunk", "keep") == "merge" && out.len() > 1 && out.last().map(|p| p.pages.len() < n).unwrap_or(false) {
                let last = out.pop().unwrap();
                out.last_mut().unwrap().pages.extend(last.pages);
            }
            out
        }
        "ranges" => {
            let spec = req.str_or("ranges", "");
            let mut out = Vec::new();
            for part in spec.split([';', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
                out.push(piece(pages::parse_ordered(part, total).map_err(|b| bad_range(b, total))?));
            }
            out
        }
        "extract" => vec![piece(pages::parse_ordered(req.str_or("pages", "all"), total).map_err(|b| bad_range(b, total))?)],
        "atPages" => {
            let points = pages::parse_set(req.str_or("splitPoints", ""), total).map_err(|b| bad_range(b, total))?;
            // "all" (empty) would cut after every page; an empty box means no cut at all.
            let points: Vec<u32> = if req.str_or("splitPoints", "").trim().is_empty() { vec![] } else { points };
            let mut out = Vec::new();
            let mut start = 1;
            for p in points.into_iter().filter(|p| *p < total) {
                out.push(piece((start..=p).collect()));
                start = p + 1;
            }
            out.push(piece((start..=total).collect()));
            out
        }
        "oddEven" => {
            let odd: Vec<u32> = all.iter().copied().filter(|p| p % 2 == 1).collect();
            let even: Vec<u32> = all.iter().copied().filter(|p| p % 2 == 0).collect();
            let w = crate::common::words::of(req.locale.as_deref());
            let named = |pages: Vec<u32>, word: &str| Piece { pages, chapter: word.into() };
            match req.str_or("oddEven", "both") {
                "odd" => vec![named(odd, w.odd)],
                "even" => vec![named(even, w.even)],
                _ => vec![named(odd, w.odd), named(even, w.even)],
            }
        }
        "byBookmarks" => by_bookmarks(doc, total, req.f64_or("bookmarkLevel", 1.0) as usize)?,
        "bySize" => by_size(ctx, doc, ids, req.f64_or("maxSize", 10_000_000.0) as u64)?,
        _ => all.iter().map(|p| piece(vec![*p])).collect(),
    })
}

/// A file per bookmark at `level`; the pages before the first one go with it.
fn by_bookmarks(doc: &Document, total: u32, level: usize) -> Result<Vec<Piece>, FileError> {
    let toc = doc.get_toc().map_err(|_| FileError::new("no_bookmarks"))?;
    let mut starts: Vec<(u32, String)> = Vec::new();
    for e in toc.toc.iter().filter(|e| e.level == level.max(1)) {
        let p = e.page as u32;
        if p >= 1 && p <= total && !starts.iter().any(|(s, _)| *s == p) {
            starts.push((p, e.title.trim().to_string()));
        }
    }
    if starts.is_empty() {
        return Err(FileError::new("no_bookmarks"));
    }
    starts.sort_by_key(|(p, _)| *p);
    starts[0].0 = 1;
    let mut out = Vec::new();
    for (i, (from, title)) in starts.iter().enumerate() {
        let to = starts.get(i + 1).map(|(p, _)| p - 1).unwrap_or(total);
        if to >= *from {
            out.push(Piece { pages: (*from..=to).collect(), chapter: safe_name(title) });
        }
    }
    Ok(out)
}

/// Fills each file with pages until the next one would pass `max`. A page that is
/// larger than `max` on its own goes into a file by itself.
fn by_size(ctx: &Ctx, doc: &Document, ids: &[ObjectId], max: u64) -> Result<Vec<Piece>, FileError> {
    // Pieces measured for real: adding up each page's size counts the fonts and pictures
    // the pages share once per page, and made pieces several times smaller than allowed.
    // Each piece grows by doubling, then the exact count is found by halving.
    let n = ids.len();
    let mut out: Vec<Piece> = Vec::new();
    let mut start = 0usize;
    while start < n {
        ctx.check_cancel()?;
        ctx.progress("*", start as f64 / n as f64 * 0.5, "read", 1, 2);
        let rest = n - start;
        let fits = |count: usize| saved_size(&mut pdf::extract_pages(doc, &ids[start..start + count], false)) <= max;
        // At least one page: a single page over the limit is a piece of its own.
        let mut good = 1usize;
        let mut step = 1usize;
        let mut bad = None;
        while good < rest {
            let c = (good + step).min(rest);
            if fits(c) {
                good = c;
                step *= 2;
            } else {
                bad = Some(c);
                break;
            }
        }
        if let Some(mut hi) = bad {
            let mut lo = good;
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            good = lo;
        }
        out.push(piece((start + 1..=start + good).map(|p| p as u32).collect()));
        start += good;
    }
    Ok(out)
}

/// The size the document would have on disk. Pages sharing images or fonts are
/// counted once each, so the sum over pages is an upper bound.
fn saved_size(doc: &mut Document) -> u64 {
    let mut buf = Vec::new();
    let opts = SaveOptions::builder().use_object_streams(true).use_xref_streams(true).compression_level(9).build();
    match doc.save_with_options(&mut buf, opts) {
        Ok(_) => buf.len() as u64,
        Err(_) => 0,
    }
}

/// A chapter title fit for a file name.
fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let joined = words.join(" ");
    joined.chars().take(80).collect::<String>().trim_end_matches(['.', ' ']).to_string()
}
