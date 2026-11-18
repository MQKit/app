//! The PDF layer shared by the PDF tools (lopdf).

use contract::{codes, FileError, FileRef, Request};
use lopdf::{Dictionary, Document, Object, ObjectId, SaveOptions};
use serde_json::{json, Map, Value};
use std::path::Path;

/// Opens a PDF, with the password from `secrets.files[<id>]` if the parent sent one.
pub fn open_file(f: &FileRef, req: &Request) -> Result<Document, FileError> {
    open(f.input_path(), req.secrets.files.get(&f.id).map(String::as_str))
}

/// The largest PDF opened. The file is read whole and then held again as objects (about
/// twice its size, more while pages are worked on); far past this the worker would run
/// out of memory and stop for every file of the batch. Scans of hundreds of pages are
/// a few hundred MB.
pub const MAX_PDF_BYTES: u64 = 1200 * 1024 * 1024;

pub fn open(path: &Path, password: Option<&str>) -> Result<Document, FileError> {
    let size = std::fs::metadata(path).map_err(crate::plugins::io_err)?.len();
    if size > MAX_PDF_BYTES {
        return Err(FileError::with(codes::FILE_TOO_LARGE, json!({ "sizeBytes": size, "maxBytes": MAX_PDF_BYTES })));
    }
    let res = match inline_encrypt_fixed(path) {
        // An encryption dictionary written inside the trailer (MuPDF does this).
        Some(bytes) => {
            let options = lopdf::LoadOptions { password: password.map(str::to_string), ..Default::default() };
            Document::load_mem_with_options(&bytes, options)
        }
        None => match password {
            Some(p) => Document::load_with_password(path, p),
            None => Document::load(path),
        },
    };
    let mut doc = res.map_err(|e| map_err(e, password.is_some()))?;
    if doc.is_encrypted() {
        // Encrypted with a non-empty user password and not yet decrypted.
        match password {
            Some(p) => doc.decrypt(p).map_err(|e| map_err(e, true))?,
            None => return Err(FileError::new(codes::PASSWORD_REQUIRED)),
        }
    }
    // A trailer that lost its /Root (a damaged end of file, the first-page trailer of a
    // linearized file not read): the catalog is found among the objects, as viewers do.
    if doc.get_pages().is_empty() {
        let catalog = doc.objects.iter().find_map(|(id, o)| {
            let d = o.as_dict().ok()?;
            (d.get(b"Type").and_then(Object::as_name).ok() == Some(b"Catalog".as_slice()) && d.has(b"Pages")).then_some(*id)
        });
        if let Some(id) = catalog {
            doc.trailer.set("Root", id);
        }
    }
    if doc.get_pages().is_empty() {
        rebuild_page_tree(&mut doc);
    }
    // A file whose pages can't be found is damaged: never "convert" it into an empty PDF.
    if doc.get_pages().is_empty() {
        return Err(FileError::with(codes::CORRUPT_FILE, json!({ "message": "no pages" })));
    }
    Ok(doc)
}

/// A page tree that is missing or broken (its object lost): the pages are gathered as
/// viewers do, in object order, the first page of a linearized file first.
fn rebuild_page_tree(doc: &mut Document) {
    let is = |o: &Object, ty: &[u8]| o.as_dict().ok().and_then(|d| d.get(b"Type").ok()).and_then(|t| t.as_name().ok()) == Some(ty);
    let mut pages: Vec<ObjectId> = doc.objects.iter().filter(|(_, o)| is(o, b"Page")).map(|(id, _)| *id).collect();
    if pages.is_empty() {
        return;
    }
    let first = doc.objects.values().find_map(|o| {
        let d = o.as_dict().ok()?;
        d.has(b"Linearized").then(|| d.get(b"O").and_then(Object::as_i64).ok()).flatten()
    });
    if let Some(at) = first.and_then(|n| pages.iter().position(|p| p.0 as i64 == n)) {
        let p = pages.remove(at);
        pages.insert(0, p);
    }
    let tree = doc.new_object_id();
    for p in &pages {
        if let Ok(d) = doc.get_dictionary_mut(*p) {
            d.set("Parent", tree);
        }
    }
    let kids: Vec<Object> = pages.iter().map(|p| Object::Reference(*p)).collect();
    doc.objects.insert(tree, Object::Dictionary(lopdf::dictionary! { "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids }));
    let root = doc.trailer.get(b"Root").and_then(Object::as_reference).ok().filter(|r| doc.get_dictionary(*r).is_ok());
    match root {
        Some(r) => {
            if let Ok(c) = doc.get_dictionary_mut(r) {
                c.set("Pages", tree);
            }
        }
        None => {
            let c = doc.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => tree });
            doc.trailer.set("Root", c);
        }
    }
}

/// lopdf finds the encryption dictionary only as an object of its own, but some writers
/// (MuPDF, so many PDFs made by scripts) put it inside the trailer: lopdf then saw no
/// encryption, or no pages. Such a file is read with a small update appended in memory
/// that moves the dictionary into an object (the file on disk is untouched).
fn inline_encrypt_fixed(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let tail_len = len.min(512 * 1024);
    f.seek(SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = Vec::with_capacity(tail_len as usize);
    f.read_to_end(&mut tail).ok()?;
    // The key itself, not /EncryptMetadata inside the dictionary.
    let at = tail
        .windows(9)
        .rposition(|w| &w[..8] == b"/Encrypt" && !w[8].is_ascii_alphanumeric())?;
    let mut i = at + 8;
    while i < tail.len() && tail[i].is_ascii_whitespace() {
        i += 1;
    }
    if !tail[i..].starts_with(b"<<") {
        return None; // a reference: lopdf reads it
    }
    let end = dict_end(&tail, i)?;
    let dict = tail[i..end].to_vec();
    let mut raw = Vec::with_capacity(len as usize + 1024);
    f.seek(SeekFrom::Start(0)).ok()?;
    f.read_to_end(&mut raw).ok()?;
    // What the new trailer needs, read from the text of the trailer that holds /Encrypt
    // (lopdf itself can't load these files).
    let dict_start = rfind(&tail[..at], b"<<")?;
    let trailer_text = String::from_utf8_lossy(&tail[dict_start..end.max(at)]).into_owned();
    let mut rest = String::from_utf8_lossy(&tail[end..]).into_owned();
    rest.truncate(rest.find("startxref").unwrap_or(rest.len()));
    let whole = format!("{trailer_text}{rest}");
    let after = |key: &str| -> Option<String> {
        let i = whole.find(key)? + key.len();
        Some(whole[i..].trim_start().to_string())
    };
    let reference = |key: &str| -> Option<String> {
        let v = after(key)?;
        let mut parts = v.split(|c: char| c.is_whitespace() || c == '/' || c == '>').filter(|p| !p.is_empty());
        let (n, g, r) = (parts.next()?, parts.next()?, parts.next()?);
        (n.chars().all(|c| c.is_ascii_digit()) && g.chars().all(|c| c.is_ascii_digit()) && r.starts_with('R')).then(|| format!("{n} {g} R"))
    };
    let root = reference("/Root")?;
    let mut extra = String::new();
    if let Some(info) = reference("/Info") {
        extra += &format!(" /Info {info}");
    }
    if let Some(v) = after("/ID") {
        if v.starts_with('[') {
            if let Some(close) = v.find(']') {
                extra += &format!(" /ID {}", &v[..=close]);
            }
        }
    }
    let size: i64 = after("/Size").and_then(|v| v.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok())?;
    let prev = {
        let sx = rfind(&raw, b"startxref")?;
        let digits: String = raw[sx + 9..].iter().skip_while(|c| c.is_ascii_whitespace()).take_while(|c| c.is_ascii_digit()).map(|c| *c as char).collect();
        digits.parse::<u64>().ok()?
    };
    let n = size;
    raw.extend_from_slice(b"\n");
    let obj_at = raw.len();
    raw.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
    raw.extend_from_slice(&dict);
    raw.extend_from_slice(b"\nendobj\n");
    let xref_at = raw.len();
    raw.extend_from_slice(format!("xref\n{n} 1\n{obj_at:010} 00000 n \ntrailer\n<< /Size {} /Root {root}{extra} /Encrypt {n} 0 R /Prev {prev} >>\nstartxref\n{xref_at}\n%%EOF\n", n + 1).as_bytes());
    Some(raw)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// The end (exclusive) of the dictionary starting at `start` ("<<"), strings skipped.
fn dict_end(b: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = start;
    while i + 1 < b.len() {
        match b[i] {
            b'(' => {
                let mut d = 0;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 1,
                        b'(' => d += 1,
                        b')' => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'<' if b[i + 1] == b'<' => {
                depth += 1;
                i += 1;
            }
            b'>' if b[i + 1] == b'>' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'<' => {
                while i < b.len() && b[i] != b'>' {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn map_err(e: lopdf::Error, had_password: bool) -> FileError {
    match e {
        lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_) => {
            FileError::new(if had_password { codes::WRONG_PASSWORD } else { codes::PASSWORD_REQUIRED })
        }
        lopdf::Error::IO(io) => crate::plugins::io_err(io),
        other => FileError::with(codes::CORRUPT_FILE, json!({ "message": other.to_string() })),
    }
}

/// The PDF opened in Windows' renderer (for pictures of its pages). Windows can't read
/// AES-256 encryption, the most common today: such a file is opened from a decrypted copy
/// in the job's own folder (it needs the password the user gave, like any other way).
pub fn windows_reader(ctx: &crate::plugins::Ctx, f: &FileRef) -> Result<crate::common::winrt::Pdf, FileError> {
    let password = ctx.req.secrets.files.get(&f.id).map(String::as_str);
    match crate::common::winrt::Pdf::open(f.input_path(), password) {
        Ok(p) => Ok(p),
        Err(first) => {
            let Ok(mut doc) = open_file(f, ctx.req) else { return Err(first) };
            if !doc.was_encrypted() {
                return Err(first);
            }
            doc.encryption_state = None;
            // The decrypted copy stays in memory: nothing unprotected is left on the disk.
            let mut plain = Vec::new();
            write(&mut doc, &mut plain, false)?;
            drop(doc);
            crate::common::winrt::Pdf::open_bytes(&plain)
        }
    }
}

/// Fast analysis (section 9.5): page count, encryption, forms, signatures, PDF/A, image share.
pub fn analyze(f: &FileRef, req: &Request) -> (&'static str, Map<String, Value>, Option<String>) {
    let mut p = Map::new();
    let doc = match open_file(f, req) {
        Ok(d) => d,
        Err(e) if e.code == codes::PASSWORD_REQUIRED || e.code == codes::WRONG_PASSWORD => {
            p.insert("needsPassword".into(), json!(true));
            p.insert("isEncrypted".into(), json!(true));
            return ("locked", p, Some(e.code));
        }
        Err(e) if e.code == codes::FILE_TOO_LARGE => return ("failed", p, Some(e.code)),
        Err(e) => return ("corrupt", p, Some(e.code)),
    };
    let pages = doc.get_pages();
    p.insert("pageCount".into(), json!(pages.len()));
    p.insert("needsPassword".into(), json!(false));
    p.insert("isEncrypted".into(), json!(doc.was_encrypted()));
    let catalog = doc.catalog().ok();
    let acro = catalog.and_then(|c| deref_dict(&doc, c.get(b"AcroForm").ok()));
    let fields = acro.and_then(|a| a.get(b"Fields").ok()).and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok()).map(|a| a.len()).unwrap_or(0);
    let sig_flags = acro.and_then(|a| a.get(b"SigFlags").ok()).and_then(|o| o.as_i64().ok()).unwrap_or(0);
    p.insert("hasForms".into(), json!(fields > 0 && sig_flags == 0));
    p.insert("hasXfa".into(), json!(acro.map(|a| a.has(b"XFA")).unwrap_or(false)));
    p.insert("isSigned".into(), json!(sig_flags & 1 == 1));
    p.insert("hasBookmarks".into(), json!(catalog.map(|c| c.has(b"Outlines")).unwrap_or(false)));
    let names = catalog.and_then(|c| deref_dict(&doc, c.get(b"Names").ok()));
    p.insert("hasAttachments".into(), json!(names.map(|n| n.has(b"EmbeddedFiles")).unwrap_or(false)));
    let is_pdfa = catalog
        .and_then(|c| c.get(b"Metadata").ok())
        .and_then(|o| o.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|o| o.as_stream().ok())
        .map(|s| String::from_utf8_lossy(&s.content).contains("pdfaid:part"))
        .unwrap_or(false);
    p.insert("isPdfA".into(), json!(is_pdfa));
    // Share of the file taken by images: tells if compression can gain much.
    let total = crate::plugins::file_size(f.input_path()).max(1);
    let img: usize = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| s.dict.get(b"Subtype").and_then(|o| o.as_name()).ok() == Some(b"Image".as_slice()))
        .map(|s| s.content.len())
        .sum();
    p.insert("imageBytesRatio".into(), json!(((img as f64 / total as f64) * 100.0).round() / 100.0));
    p.insert("imageBytes".into(), json!(img));
    // Largest page side, in points (for the image resolution heuristics).
    let max_side = pages.values().filter_map(|id| page_box(&doc, *id)).map(|(w, h)| w.max(h)).fold(0.0f64, f64::max);
    p.insert("maxPageSidePt".into(), json!(max_side.round()));
    ("ok", p, None)
}

fn deref_dict<'a>(doc: &'a Document, o: Option<&'a Object>) -> Option<&'a Dictionary> {
    let o = o?;
    doc.dereference(o).ok()?.1.as_dict().ok()
}

/// MediaBox of a page (inherited if needed): (width, height) in points.
pub fn page_box(doc: &Document, page: ObjectId) -> Option<(f64, f64)> {
    let mut id = page;
    for _ in 0..32 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(mb) = d.get(b"MediaBox") {
            let arr = doc.dereference(mb).ok()?.1.as_array().ok()?;
            let n: Vec<f64> = arr.iter().filter_map(|o| num(doc, o)).collect();
            if n.len() == 4 {
                return Some(((n[2] - n[0]).abs(), (n[3] - n[1]).abs()));
            }
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

pub fn num(doc: &Document, o: &Object) -> Option<f64> {
    match doc.dereference(o).ok()?.1 {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

/// Copies the attributes a page inherits from its parents into the page itself,
/// so the page can be moved under another Pages node.
pub fn materialize_inherited(doc: &mut Document, page: ObjectId) {
    const KEYS: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
    let mut found: Vec<(&[u8], Object)> = Vec::new();
    if let Ok(d) = doc.get_dictionary(page) {
        let mut parent = d.get(b"Parent").and_then(Object::as_reference).ok();
        for _ in 0..32 {
            let Some(pid) = parent else { break };
            let Ok(pd) = doc.get_dictionary(pid) else { break };
            for k in KEYS {
                if !d.has(k) && !found.iter().any(|(fk, _)| *fk == k) {
                    if let Ok(v) = pd.get(k) {
                        found.push((k, v.clone()));
                    }
                }
            }
            parent = pd.get(b"Parent").and_then(Object::as_reference).ok();
        }
    }
    if let Ok(d) = doc.get_dictionary_mut(page) {
        for (k, v) in found {
            d.set(k, v);
        }
    }
}

/// A new document with only the given pages (in that order, repeats allowed) and
/// the objects they need. Links and annotations that point at other pages are left
/// dangling (viewers treat them as empty). Call `materialize_inherited` on the pages first.
pub fn extract_pages(src: &Document, pages: &[ObjectId], keep_info: bool) -> Document {
    use std::collections::HashSet;
    let wanted: HashSet<ObjectId> = pages.iter().copied().collect();
    // The page tree and the pages left out are never copied.
    let blocked: HashSet<ObjectId> = src
        .objects
        .iter()
        .filter_map(|(id, o)| {
            let ty = o.as_dict().ok()?.get(b"Type").and_then(Object::as_name).ok()?;
            match ty {
                b"Pages" => Some(*id),
                b"Page" if !wanted.contains(id) => Some(*id),
                _ => None,
            }
        })
        .collect();
    let root = src.trailer.get(b"Root").and_then(Object::as_reference).ok();
    let mut out = Document::with_version("1.7");
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut stack: Vec<ObjectId> = pages.to_vec();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || blocked.contains(&id) || Some(id) == root {
            continue;
        }
        if let Ok(obj) = src.get_object(id) {
            collect_refs(obj, &mut stack);
            out.objects.insert(id, obj.clone());
        }
    }
    let mut next = src.max_id + 1;
    let pages_id: ObjectId = (next, 0);
    next += 1;
    let mut kids = Vec::with_capacity(pages.len());
    let mut used = HashSet::new();
    for p in pages {
        // A page used twice needs its own page object.
        let id = if used.insert(*p) {
            *p
        } else {
            let id = (next, 0);
            next += 1;
            if let Some(o) = out.objects.get(p).cloned() {
                out.objects.insert(id, o);
            }
            id
        };
        if let Some(Object::Dictionary(d)) = out.objects.get_mut(&id) {
            d.set("Parent", pages_id);
            d.remove(b"StructParents");
        }
        kids.push(Object::Reference(id));
    }
    drop_dead_links(&mut out, &kids, &blocked);
    let count = kids.len() as i64;
    out.objects.insert(pages_id, Object::Dictionary(lopdf::dictionary! { "Type" => "Pages", "Count" => count, "Kids" => kids.clone() }));
    let catalog_id: ObjectId = (next, 0);
    next += 1;
    let mut catalog = lopdf::dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    // What belongs to the document rather than to a page: bookmarks, form fields, layers,
    // attachments. They were left behind, so pages lost their bookmarks and forms.
    let same_pages: Vec<ObjectId> = src.get_pages().values().copied().collect();
    let whole = same_pages.iter().all(|p| wanted.contains(p));
    let in_order = whole && pages == same_pages.as_slice();
    let kept: HashSet<ObjectId> = kids.iter().filter_map(|k| k.as_reference().ok()).chain(pages.iter().copied()).collect();
    if let Ok(src_cat) = src.catalog() {
        let mut parts = DocParts { src, blocked: &blocked, root, seen: &mut seen, kept: &kept, next: &mut next };
        for key in [b"OCProperties".as_slice(), b"Lang", b"ViewerPreferences", b"PageLayout", b"PageMode", b"Extensions"] {
            if let Ok(v) = src_cat.get(key) {
                parts.copy(&mut out, v);
                catalog.set(key.to_vec(), v.clone());
            }
        }
        if let Some(outline) = parts.outline(&mut out, src_cat) {
            catalog.set("Outlines", outline);
        }
        if let Some(form) = parts.form(&mut out, src_cat) {
            catalog.set("AcroForm", form);
        }
        // The same document, pages reordered or taken out: its attachments and scripts stay.
        let mut entries: Vec<&[u8]> = vec![];
        if whole {
            entries.extend([b"AF".as_slice(), b"Collection", b"URI"]);
        }
        if whole && keep_info {
            entries.push(b"Metadata");
        }
        if in_order {
            entries.push(b"PageLabels");
        }
        for key in entries {
            if let Ok(v) = src_cat.get(key) {
                parts.copy(&mut out, v);
                catalog.set(key.to_vec(), v.clone());
            }
        }
        if whole {
            if let Some(Object::Dictionary(mut names)) = src_cat.get(b"Names").ok().and_then(|o| src.dereference(o).ok()).map(|(_, o)| o.clone()) {
                names.remove(b"Dests"); // already turned into explicit destinations
                if !names.is_empty() {
                    let v = Object::Dictionary(names);
                    parts.copy(&mut out, &v);
                    catalog.set("Names", v);
                }
            }
            if let Ok(v) = src_cat.get(b"OpenAction") {
                if parts.dest_kept(v) {
                    parts.copy(&mut out, v);
                    catalog.set("OpenAction", v.clone());
                }
            }
        }
    }
    out.objects.insert(catalog_id, Object::Dictionary(catalog));
    out.trailer.set("Root", catalog_id);
    if keep_info {
        if let Some(info) = src.trailer.get(b"Info").ok().and_then(|o| src.dereference(o).ok()).and_then(|(_, o)| o.as_dict().ok()) {
            let id = (next, 0);
            next += 1;
            out.objects.insert(id, Object::Dictionary(info.clone()));
            out.trailer.set("Info", id);
        }
    }
    out.max_id = next;
    // Objects only reachable from what was left behind (other pages' form fields) go.
    out.prune_objects();
    // Pages of a protected document stay protected (same passwords, same permissions):
    // the encryption keys depend on the document's ID, so it comes along.
    if src.encryption_state.is_some() {
        out.encryption_state = src.encryption_state.clone();
        if let Ok(id) = src.trailer.get(b"ID") {
            out.trailer.set("ID", id.clone());
        }
    }
    out
}

/// Named destinations (`/Dest (chapter.1)`, a GoTo action's `/D /sec2`) point into the
/// catalog's name tree, which is left behind when pages move to another document (merge,
/// split): bookmarks and links went dead. Each one becomes the explicit destination it names.
pub fn resolve_named_dests(doc: &mut Document) {
    use std::collections::HashMap;
    let mut names: HashMap<Vec<u8>, Object> = HashMap::new();
    let explicit = |doc: &Document, o: &Object| -> Option<Object> {
        let (_, v) = doc.dereference(o).ok()?;
        match v {
            Object::Array(_) => Some(v.clone()),
            Object::Dictionary(d) => d.get(b"D").ok().and_then(|x| doc.dereference(x).ok()).map(|(_, x)| x.clone()).filter(|x| x.as_array().is_ok()),
            _ => None,
        }
    };
    let Ok(catalog) = doc.catalog() else { return };
    // PDF 1.1: a dictionary of names.
    if let Some(d) = catalog.get(b"Dests").ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_dict().ok()) {
        for (k, v) in d.iter() {
            if let Some(e) = explicit(doc, v) {
                names.insert(k.clone(), e);
            }
        }
    }
    // PDF 1.2 on: a name tree of strings.
    let tree = catalog
        .get(b"Names")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok())
        .and_then(|n| n.get(b"Dests").ok())
        .and_then(|o| doc.dereference(o).ok())
        .map(|(_, o)| o.clone());
    let mut stack: Vec<Object> = tree.into_iter().collect();
    let mut guard = 0;
    while let Some(node) = stack.pop() {
        guard += 1;
        if guard > 200_000 {
            break;
        }
        let Ok(d) = node.as_dict() else { continue };
        if let Ok(a) = d.get(b"Names").and_then(|o| doc.dereference(o).map(|(_, o)| o)).and_then(Object::as_array) {
            for pair in a.chunks(2) {
                if let [Object::String(k, _), v] = pair {
                    if let Some(e) = explicit(doc, v) {
                        names.insert(k.clone(), e);
                    }
                }
            }
        }
        if let Ok(kids) = d.get(b"Kids").and_then(|o| doc.dereference(o).map(|(_, o)| o)).and_then(Object::as_array) {
            for k in kids {
                if let Ok((_, o)) = doc.dereference(k) {
                    stack.push(o.clone());
                }
            }
        }
    }
    if names.is_empty() {
        return;
    }
    let lookup = |o: &Object| -> Option<Object> {
        match o {
            Object::Name(n) | Object::String(n, _) => names.get(n).cloned(),
            _ => None,
        }
    };
    for obj in doc.objects.values_mut() {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &mut s.dict,
            _ => continue,
        };
        // Bookmarks and link annotations: /Dest. GoTo actions: /D.
        if let Some(e) = dict.get(b"Dest").ok().and_then(lookup) {
            dict.set("Dest", e);
        }
        let goto = dict.get(b"S").and_then(Object::as_name).ok() == Some(b"GoTo".as_slice());
        if goto {
            if let Some(e) = dict.get(b"D").ok().and_then(lookup) {
                dict.set("D", e);
            }
        }
        // An action written inline in a bookmark or link: /A << /S /GoTo /D (name) >>.
        if let Ok(Object::Dictionary(a)) = dict.get_mut(b"A") {
            if a.get(b"S").and_then(Object::as_name).ok() == Some(b"GoTo".as_slice()) {
                if let Some(e) = a.get(b"D").ok().and_then(lookup) {
                    a.set("D", e);
                }
            }
        }
    }
}

/// Links to pages that didn't come along would point at nothing (a broken reference):
/// they are taken off the pages that have them.
fn drop_dead_links(out: &mut Document, kids: &[Object], blocked: &std::collections::HashSet<ObjectId>) {
    let target = |o: &Object| -> Option<ObjectId> {
        let arr = o.as_array().ok()?;
        arr.first()?.as_reference().ok()
    };
    for kid in kids {
        let Ok(page) = kid.as_reference() else { continue };
        let annots: Vec<Object> = match out.get_dictionary(page).ok().and_then(|d| d.get(b"Annots").ok()).cloned() {
            Some(Object::Array(a)) => a,
            Some(Object::Reference(id)) => out.get_object(id).ok().and_then(|o| o.as_array().ok()).cloned().unwrap_or_default(),
            _ => continue,
        };
        let keep: Vec<Object> = annots
            .iter()
            .filter(|a| {
                let dict = match a {
                    Object::Reference(id) => out.get_dictionary(*id).ok(),
                    Object::Dictionary(d) => Some(d),
                    _ => None,
                };
                let Some(d) = dict else { return true };
                if d.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Link".as_slice()) {
                    return true;
                }
                let dest = d.get(b"Dest").ok().cloned().or_else(|| {
                    d.get(b"A").ok().and_then(|a| match a {
                        Object::Dictionary(ad) => ad.get(b"D").ok().cloned(),
                        Object::Reference(id) => out.get_dictionary(*id).ok().and_then(|ad| ad.get(b"D").ok().cloned()),
                        _ => None,
                    })
                });
                match dest.as_ref().and_then(target) {
                    Some(pid) => !blocked.contains(&pid) && out.objects.contains_key(&pid),
                    None => true,
                }
            })
            .cloned()
            .collect();
        if keep.len() != annots.len() {
            if let Ok(d) = out.get_dictionary_mut(page) {
                d.set("Annots", keep);
            }
        }
    }
}

/// Copies the document-level parts that go with the extracted pages.
struct DocParts<'a> {
    src: &'a Document,
    blocked: &'a std::collections::HashSet<ObjectId>,
    root: Option<ObjectId>,
    seen: &'a mut std::collections::HashSet<ObjectId>,
    kept: &'a std::collections::HashSet<ObjectId>,
    next: &'a mut u32,
}

struct OutlineItem {
    dict: lopdf::Dictionary,
    open: bool,
    kids: Vec<OutlineItem>,
}

impl DocParts<'_> {
    /// Copies the objects `o` refers to (and theirs), pages left out excepted.
    fn copy(&mut self, out: &mut Document, o: &Object) {
        let mut stack = Vec::new();
        collect_refs(o, &mut stack);
        while let Some(id) = stack.pop() {
            if !self.seen.insert(id) || self.blocked.contains(&id) || Some(id) == self.root {
                continue;
            }
            if let Ok(obj) = self.src.get_object(id) {
                collect_refs(obj, &mut stack);
                out.objects.insert(id, obj.clone());
            }
        }
    }

    fn new_id(&mut self) -> ObjectId {
        let id = (*self.next, 0);
        *self.next += 1;
        id
    }

    /// The page a destination or GoTo action goes to (None: not a page in this file).
    fn dest_page(&self, o: &Object) -> Option<ObjectId> {
        let (_, v) = self.src.dereference(o).ok()?;
        match v {
            Object::Array(a) => a.first()?.as_reference().ok(),
            Object::Dictionary(d) if d.get(b"S").and_then(Object::as_name).ok() == Some(b"GoTo".as_slice()) => self.dest_page(d.get(b"D").ok()?),
            _ => None,
        }
    }

    /// Whether a destination or action still has somewhere to go.
    fn dest_kept(&self, o: &Object) -> bool {
        self.dest_page(o).is_none_or(|p| self.kept.contains(&p))
    }

    /// The bookmarks that lead to the pages kept (and the ones grouping them).
    fn outline(&mut self, out: &mut Document, cat: &lopdf::Dictionary) -> Option<ObjectId> {
        let top = self.src.dereference(cat.get(b"Outlines").ok()?).ok()?.1.as_dict().ok()?;
        let mut visited = std::collections::HashSet::new();
        let items = self.read_items(top.get(b"First").and_then(Object::as_reference).ok(), &mut visited, 0);
        if items.is_empty() {
            return None;
        }
        let id = self.new_id();
        let (first, last, count) = self.write_items(out, items, id);
        out.objects.insert(id, Object::Dictionary(lopdf::dictionary! { "Type" => "Outlines", "First" => first, "Last" => last, "Count" => count }));
        Some(id)
    }

    fn read_items(&self, mut cur: Option<ObjectId>, visited: &mut std::collections::HashSet<ObjectId>, depth: u32) -> Vec<OutlineItem> {
        let mut items = vec![];
        while let Some(id) = cur {
            if depth > 64 || !visited.insert(id) {
                break;
            }
            let Ok(d) = self.src.get_dictionary(id) else { break };
            cur = d.get(b"Next").and_then(Object::as_reference).ok();
            let kids = self.read_items(d.get(b"First").and_then(Object::as_reference).ok(), visited, depth + 1);
            let target = d.get(b"Dest").or_else(|_| d.get(b"A")).ok();
            let goes = target.is_none_or(|t| self.dest_kept(t));
            if !goes && kids.is_empty() {
                continue;
            }
            let mut dict = lopdf::Dictionary::new();
            for key in [b"Title".as_slice(), b"C", b"F"] {
                if let Ok(v) = d.get(key) {
                    dict.set(key.to_vec(), v.clone());
                }
            }
            if goes {
                for key in [b"Dest".as_slice(), b"A"] {
                    if let Ok(v) = d.get(key) {
                        dict.set(key.to_vec(), v.clone());
                    }
                }
            }
            let open = d.get(b"Count").and_then(Object::as_i64).map(|c| c > 0).unwrap_or(false);
            items.push(OutlineItem { dict, open, kids });
        }
        items
    }

    /// Writes items under `parent`: (first, last, count of the items shown when open).
    fn write_items(&mut self, out: &mut Document, items: Vec<OutlineItem>, parent: ObjectId) -> (ObjectId, ObjectId, i64) {
        let ids: Vec<ObjectId> = items.iter().map(|_| self.new_id()).collect();
        let mut shown = 0i64;
        let n = ids.len();
        for (i, mut item) in items.into_iter().enumerate() {
            let values: Vec<Object> = item.dict.iter().map(|(_, v)| v.clone()).collect();
            for v in &values {
                self.copy(out, v);
            }
            item.dict.set("Parent", parent);
            if i > 0 {
                item.dict.set("Prev", ids[i - 1]);
            }
            if i + 1 < n {
                item.dict.set("Next", ids[i + 1]);
            }
            shown += 1;
            if !item.kids.is_empty() {
                let (f, l, c) = self.write_items(out, item.kids, ids[i]);
                item.dict.set("First", f);
                item.dict.set("Last", l);
                item.dict.set("Count", if item.open { c } else { -c });
                if item.open {
                    shown += c;
                }
            }
            out.objects.insert(ids[i], Object::Dictionary(item.dict));
        }
        (ids[0], ids[n - 1], shown)
    }

    /// The form, with the fields that have a widget on the pages kept.
    fn form(&mut self, out: &mut Document, cat: &lopdf::Dictionary) -> Option<Object> {
        let mut form = self.src.dereference(cat.get(b"AcroForm").ok()?).ok()?.1.as_dict().ok()?.clone();
        let widgets: std::collections::HashSet<ObjectId> = self
            .kept
            .iter()
            .filter_map(|p| out.get_dictionary(*p).ok())
            .filter_map(|d| d.get(b"Annots").ok())
            .filter_map(|a| out.dereference(a).ok().and_then(|(_, a)| a.as_array().ok()).cloned())
            .flatten()
            .filter_map(|a| a.as_reference().ok())
            .collect();
        let fields = form.get(b"Fields").ok().and_then(|o| self.src.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok()).cloned().unwrap_or_default();
        let kept: Vec<Object> = fields.into_iter().filter(|f| f.as_reference().is_ok_and(|id| self.field(out, id, &widgets, 0))).collect();
        if kept.is_empty() {
            return None;
        }
        form.set("Fields", kept);
        form.remove(b"XFA"); // its own copy of the form, for all the pages
        let v = Object::Dictionary(form);
        self.copy(out, &v);
        Some(v)
    }

    /// Whether a field has a widget on a page kept; its other widgets are taken off.
    fn field(&mut self, out: &mut Document, id: ObjectId, widgets: &std::collections::HashSet<ObjectId>, depth: u32) -> bool {
        if widgets.contains(&id) {
            return true;
        }
        let Ok(d) = self.src.get_dictionary(id) else { return false };
        let kids = d.get(b"Kids").ok().and_then(|o| self.src.dereference(o).ok()).and_then(|(_, o)| o.as_array().ok()).cloned().unwrap_or_default();
        if kids.is_empty() || depth > 32 {
            return false;
        }
        let kept: Vec<Object> = kids.into_iter().filter(|k| k.as_reference().is_ok_and(|kid| self.field(out, kid, widgets, depth + 1))).collect();
        if kept.is_empty() {
            return false;
        }
        let mut d = d.clone();
        d.set("Kids", kept);
        let mut rest = d.clone();
        rest.remove(b"Kids");
        rest.remove(b"Parent");
        self.copy(out, &Object::Dictionary(rest));
        self.seen.insert(id);
        out.objects.insert(id, Object::Dictionary(d));
        true
    }
}

fn collect_refs(o: &Object, stack: &mut Vec<ObjectId>) {
    match o {
        Object::Reference(id) => stack.push(*id),
        Object::Array(a) => a.iter().for_each(|x| collect_refs(x, stack)),
        Object::Dictionary(d) => d.iter().for_each(|(_, v)| collect_refs(v, stack)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| collect_refs(v, stack)),
        _ => {}
    }
}

/// Text string for the PDF (UTF-16BE with BOM when not ASCII).
pub fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, lopdf::StringFormat::Hexadecimal)
    }
}

/// Saves with object and xref streams (smaller), optionally linearized.
pub fn save(doc: &mut Document, path: &Path, linearize: bool) -> Result<(), FileError> {
    // A protected document opened with its password stays protected, with the same
    // passwords and permissions. (To remove them, the password tool clears the state.)
    let file = std::fs::File::create(path).map_err(crate::plugins::io_err)?;
    let mut w = std::io::BufWriter::new(file);
    write(doc, &mut w, linearize)?;
    std::io::Write::flush(&mut w).map_err(crate::plugins::io_err)?;
    Ok(())
}

/// [`save`] into any writer.
pub fn write(doc: &mut Document, w: &mut impl std::io::Write, linearize: bool) -> Result<(), FileError> {
    if let Some(state) = doc.encryption_state.clone() {
        doc.encrypt(&state).map_err(FileError::engine)?;
    }
    // Object streams are only compressed when given a level (lopdf's default is 0: stored).
    let opts = SaveOptions::builder()
        .use_object_streams(!linearize)
        .use_xref_streams(!linearize)
        .linearize(linearize)
        .compression_level(9)
        .build();
    // A full disk or a folder without rights is said as such, not as a failed conversion.
    doc.save_with_options(w, opts).map_err(crate::plugins::io_err)
}

/// A page box (`MediaBox`, `CropBox`), looked up the page tree when the page inherits it.
fn page_rect(doc: &Document, page: ObjectId, key: &[u8]) -> Option<(f64, f64, f64, f64)> {
    let mut id = page;
    for _ in 0..32 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(o) = d.get(key) {
            let a = doc.dereference(o).ok()?.1.as_array().ok()?;
            if a.len() == 4 {
                let v: Vec<f64> = a.iter().map(|o| num(doc, o).unwrap_or(0.0)).collect();
                return Some((v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])));
            }
            return None;
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

pub fn media_box(doc: &Document, page: ObjectId) -> (f64, f64, f64, f64) {
    page_rect(doc, page, b"MediaBox").unwrap_or((0.0, 0.0, 612.0, 792.0))
}

/// The part of the page a reader shows: the crop box within the media box.
pub fn visible_box(doc: &Document, page: ObjectId) -> (f64, f64, f64, f64) {
    let m = media_box(doc, page);
    match page_rect(doc, page, b"CropBox") {
        Some(c) => {
            let r = (c.0.max(m.0), c.1.max(m.1), c.2.min(m.2), c.3.min(m.3));
            if r.2 - r.0 > 1.0 && r.3 - r.1 > 1.0 { r } else { m }
        }
        None => m,
    }
}

/// The page's /Rotate (0, 90, 180, 270), inherited from the page tree when not on the page.
pub fn rotation(doc: &Document, page: ObjectId) -> i64 {
    let mut id = page;
    for _ in 0..32 {
        let Ok(d) = doc.get_dictionary(id) else { break };
        if let Ok(r) = d.get(b"Rotate").and_then(|o| doc.dereference(o).map(|(_, o)| o)).and_then(Object::as_i64) {
            return r.rem_euclid(360) / 90 * 90;
        }
        let Some(parent) = d.get(b"Parent").ok().and_then(|o| o.as_reference().ok()) else { break };
        id = parent;
    }
    0
}

/// The page as it is shown: width and height in points of its visible part, after /Rotate.
pub fn displayed_size(doc: &Document, page: ObjectId) -> (f64, f64) {
    let (x0, y0, x1, y1) = visible_box(doc, page);
    let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
    if rotation(doc, page) % 180 == 90 { (h, w) } else { (w, h) }
}

/// The `cm` operator text that maps the page as shown (its visible part, origin bottom-left,
/// after /Rotate) to the page's own space, for drawing in shown coordinates.
pub fn display_cm(doc: &Document, page: ObjectId) -> String {
    let (x0, y0, x1, y1) = visible_box(doc, page);
    let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
    let (a, b, c, d, e, f) = match rotation(doc, page) {
        90 => (0.0, 1.0, -1.0, 0.0, w, 0.0),
        180 => (-1.0, 0.0, 0.0, -1.0, w, h),
        270 => (0.0, -1.0, 1.0, 0.0, 0.0, h),
        _ => (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
    };
    format!("{a} {b} {c} {d} {:.3} {:.3} cm", e + x0.min(x1), f + y0.min(y1))
}

/// Draws `content` on top of the page: the page's own content is wrapped in q/Q so its
/// state can't leak into ours; `resources` are (category, name, object) added for this page only.
pub fn append_content(doc: &mut Document, page: ObjectId, content: Vec<u8>, resources: &[(&str, &str, Object)]) -> Result<(), FileError> {
    let before = doc.add_object(lopdf::Stream::new(Dictionary::new(), b"q
".to_vec()));
    let after = doc.add_object(lopdf::Stream::new(Dictionary::new(), b"Q
".to_vec()));
    let mut stream = lopdf::Stream::new(Dictionary::new(), content);
    if stream.content.len() > 256 {
        let _ = stream.compress(); // a text layer of a few thousand words shrinks ~8×
    }
    let ours = doc.add_object(stream);
    materialize_inherited(doc, page);
    let page_dict = doc.get_dictionary_mut(page).map_err(FileError::engine)?;
    let mut contents: Vec<Object> = match page_dict.get(b"Contents") {
        Ok(Object::Array(a)) => a.clone(),
        Ok(o) => vec![o.clone()],
        Err(_) => vec![],
    };
    contents.insert(0, before.into());
    contents.push(after.into());
    contents.push(ours.into());
    page_dict.set("Contents", contents);
    let res = page_dict.get(b"Resources").ok().cloned();
    let mut res_dict = match &res {
        Some(Object::Reference(id)) => doc.get_dictionary(*id).cloned().unwrap_or_default(),
        Some(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    for (cat, name, obj) in resources {
        let mut sub = match res_dict.get(cat.as_bytes()) {
            Ok(Object::Reference(id)) => doc.get_dictionary(*id).cloned().unwrap_or_default(),
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => Dictionary::new(),
        };
        sub.set(*name, obj.clone());
        res_dict.set(*cat, sub);
    }
    // A copy for this page, so other pages sharing the resources stay as they were.
    let new_res = doc.add_object(res_dict);
    doc.get_dictionary_mut(page).map_err(FileError::engine)?.set("Resources", new_res);
    Ok(())
}

