//! Tool 3: merge PDFs, in the user's order, with optional page ranges per file.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::pdf;
use contract::{codes, opt, pages, t, FileError, FileRef, Request};
use lopdf::{dictionary, Document, Object, ObjectId};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct PdfMerge;

impl Plugin for PdfMerge {
    fn id(&self) -> &'static str {
        "pdf-merge"
    }

    fn manifest(&self) -> Value {
        json!({
            "name": t("Unire PDF", "Merge PDF"),
            "summary": t("Mai multe în unul", "Many into one"),
            "keywords": { "ro": ["combină", "lipește", "uneste", "împreună", "concatenează"], "en": ["combine", "join", "together", "concatenate"] },
            "category": "pdf",
            "icon": "files",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "combined", "ordered": true, "minFiles": 2,
                       "itemFields": [
                           { "key": "pages", "type": "pages", "syntax": "ordered", "default": "all",
                             "label": t("Pagini", "Pages"), "placeholder": t("toate", "all"),
                             "help": t("De exemplu 1-3, 5, z (ultima)", "For example 1-3, 5, z (last)") }
                       ] },
            "output": { "kind": "single", "extensions": ["pdf"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["needsPassword", "pageCount", "hasForms", "isSigned"] },
            "ui": { "preview": "none", "estimate": false, "inputList": "standard" },
            "resources": { "class": "light", "diskFactor": 1.2 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "bookmarks", "type": "select", "label": t("Semne de carte", "Bookmarks"), "default": "perFile",
                  "options": [ opt("perFile", "Unul pentru fiecare fișier", "One for each file"), opt("none", "Fără", "None") ] },
                { "key": "keepBookmarks", "type": "toggle", "advanced": true, "group": "more", "default": true,
                  "label": t("Păstrează semnele de carte ale fiecărui fișier", "Keep each file's own bookmarks"),
                  "showIf": { "field": "bookmarks", "eq": "perFile" } },
                { "key": "removeMetadata", "type": "toggle", "advanced": true, "group": "more", "default": false,
                  "label": t("Fără titlu și autor în fișierul nou", "No title and author in the new file") }
            ],
            "rules": [
                { "id": "forms_lost", "when": { "file": "hasForms", "eq": true }, "severity": "warning",
                  "message": t("Câmpurile de formular nu vor mai putea fi completate în fișierul unit.", "Form fields won't be fillable in the merged file.") },
                { "id": "signature_lost", "when": { "file": "isSigned", "eq": true }, "severity": "confirm",
                  "message": t("Semnăturile digitale nu vor fi valide în fișierul unit.", "Digital signatures won't be valid in the merged file.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_unit", "{name}_merged") } },
            "errors": {}
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn run(&self, ctx: &Ctx) -> Result<(), FileError> {
        let files = &ctx.req.files;
        ctx.run_one("*", || merge(ctx, files));
        Ok(())
    }
}

/// (First, Last, Count) of a document's outline, if it has one.
fn own_outline(doc: &Document) -> Option<(ObjectId, ObjectId, i64)> {
    let o = doc.catalog().ok()?.get(b"Outlines").ok()?;
    let d = doc.dereference(o).ok()?.1.as_dict().ok()?;
    let first = d.get(b"First").and_then(Object::as_reference).ok()?;
    let last = d.get(b"Last").and_then(Object::as_reference).ok()?;
    let count = d.get(b"Count").and_then(Object::as_i64).unwrap_or(1);
    Some((first, last, count))
}

fn file_title(f: &FileRef) -> String {
    f.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn merge(ctx: &Ctx, files: &[FileRef]) -> Result<Value, FileError> {
    let mut out = Document::with_version("1.7");
    let mut next_id = 1u32;
    // (first page id of each file, title, the file's own outline: first, last, count)
    let mut marks: Vec<(ObjectId, String, Option<(ObjectId, ObjectId, i64)>)> = Vec::new();
    let keep_own = ctx.req.bool_or("keepBookmarks", true);
    let mut all_pages: Vec<ObjectId> = Vec::new();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut in_bytes = 0u64;

    for (i, f) in files.iter().enumerate() {
        ctx.check_cancel()?;
        ctx.progress("*", i as f64 / files.len() as f64 * 0.8, "merge", 1, 2);
        in_bytes += file_size(f.input_path());
        let mut doc = pdf::open_file(f, ctx.req).map_err(|mut e| {
            e.details = Some(json!({ "file": f.id }));
            e
        })?;
        pdf::resolve_named_dests(&mut doc);
        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;
        let page_map = doc.get_pages();
        let count = page_map.len() as u32;
        let spec = f.item.get("pages").and_then(Value::as_str).unwrap_or("all");
        let wanted = pages::parse_ordered(spec, count)
            .map_err(|bad| FileError::with(codes::RANGE_OUT_OF_BOUNDS, json!({ "file": f.id, "part": bad, "pageCount": count })))?;
        let ids: Vec<ObjectId> = page_map.values().copied().collect();
        for p in &ids {
            pdf::materialize_inherited(&mut doc, *p);
        }
        let mut used = std::collections::HashSet::new();
        let mut first = None;
        for n in wanted {
            let src = ids[(n - 1) as usize];
            // A page used twice needs its own page object.
            let pid = if used.insert(src) {
                src
            } else {
                let dict = doc.get_dictionary(src).map_err(FileError::engine)?.clone();
                let new = (next_id, 0);
                next_id += 1;
                objects.insert(new, Object::Dictionary(dict));
                new
            };
            first.get_or_insert(pid);
            all_pages.push(pid);
        }
        // The file's own bookmarks, nested under its entry.
        let own = if keep_own { own_outline(&doc) } else { None };
        if let Some(fp) = first {
            marks.push((fp, file_title(f), own));
        }
        // Keep everything except the source catalog and page tree nodes.
        let catalog = doc.trailer.get(b"Root").and_then(Object::as_reference).ok();
        for (id, obj) in doc.objects.into_iter() {
            if Some(id) == catalog {
                continue;
            }
            if let Object::Dictionary(d) = &obj {
                if d.get(b"Type").and_then(Object::as_name).ok() == Some(b"Pages".as_slice()) {
                    continue;
                }
            }
            objects.insert(id, obj);
        }
    }

    // The same logo, font or picture in several files is kept once.
    dedup(&mut objects);

    let pages_id: ObjectId = (next_id, 0);
    next_id += 1;
    for p in &all_pages {
        if let Some(Object::Dictionary(d)) = objects.get_mut(p) {
            d.set("Parent", pages_id);
            d.remove(b"StructParents");
        }
    }
    objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => all_pages.len() as i64,
            "Kids" => all_pages.iter().map(|p| Object::Reference(*p)).collect::<Vec<_>>(),
        }),
    );
    let catalog_id: ObjectId = (next_id, 0);
    next_id += 1;
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };

    if ctx.req.str_or("bookmarks", "perFile") == "perFile" && !marks.is_empty() {
        let outlines_id: ObjectId = (next_id, 0);
        next_id += 1;
        let item_ids: Vec<ObjectId> = (0..marks.len()).map(|i| (next_id + i as u32, 0)).collect();
        next_id += marks.len() as u32;
        for (i, (page, title, own)) in marks.iter().enumerate() {
            let mut d = dictionary! {
                "Title" => pdf::text_string(title),
                "Parent" => outlines_id,
                "Dest" => vec![Object::Reference(*page), Object::Name(b"Fit".to_vec())],
            };
            if let Some((first, last, count)) = own {
                d.set("First", *first);
                d.set("Last", *last);
                // negative: closed, so long outlines don't flood the panel
                d.set("Count", -count.abs().max(1));
                // their top level now hangs under this entry
                let mut cur = Some(*first);
                let mut guard = 0;
                while let Some(id) = cur {
                    guard += 1;
                    if guard > 100_000 {
                        break;
                    }
                    let next = match objects.get_mut(&id) {
                        Some(Object::Dictionary(od)) => {
                            od.set("Parent", item_ids[i]);
                            od.get(b"Next").and_then(Object::as_reference).ok()
                        }
                        _ => None,
                    };
                    cur = next;
                }
            }
            if i > 0 {
                d.set("Prev", item_ids[i - 1]);
            }
            if i + 1 < item_ids.len() {
                d.set("Next", item_ids[i + 1]);
            }
            objects.insert(item_ids[i], Object::Dictionary(d));
        }
        objects.insert(
            outlines_id,
            Object::Dictionary(dictionary! {
                "Type" => "Outlines",
                "First" => item_ids[0],
                "Last" => *item_ids.last().unwrap(),
                "Count" => item_ids.len() as i64,
            }),
        );
        catalog.set("Outlines", outlines_id);
        catalog.set("PageMode", Object::Name(b"UseOutlines".to_vec()));
    }
    objects.insert(catalog_id, Object::Dictionary(catalog));

    out.objects = objects;
    out.max_id = next_id;
    out.trailer.set("Root", catalog_id);
    if !ctx.req.bool_or("removeMetadata", false) {
        let info = out.add_object(dictionary! { "Producer" => Object::string_literal("MQKit") });
        out.trailer.set("Info", info);
    }
    ctx.progress("*", 0.85, "restructure", 2, 2);
    out.prune_objects();
    out.compress();
    let dir = ctx.file_dir("merged")?;
    let path = dir.join("result.pdf");
    pdf::save(&mut out, &path, false)?;
    ctx.progress("*", 1.0, "restructure", 2, 2);
    ctx.emit_output("*", &path, "pdf", json!({ "pages": all_pages.len(), "files": files.len() }), in_bytes);
    Ok(Value::Null)
}

/// Objects that are byte-for-byte the same (streams: images, fonts, contents; and font,
/// font descriptor and graphics state dictionaries) become one, references rewritten. A few
/// rounds, since a font is only the same once its font file has been merged. Pages,
/// annotations and bookmarks are never merged: each must stay its own object.
fn dedup(objects: &mut BTreeMap<ObjectId, Object>) {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    for _ in 0..4 {
        let mut seen: HashMap<u64, Vec<ObjectId>> = HashMap::new();
        let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();
        for (id, obj) in objects.iter() {
            let key = match obj {
                Object::Stream(st) if st.content.len() >= 64 => {
                    let mut h = DefaultHasher::new();
                    format!("{:?}", st.dict).hash(&mut h);
                    st.content.hash(&mut h);
                    h.finish()
                }
                Object::Dictionary(d) => {
                    let ty = d.get(b"Type").and_then(Object::as_name).unwrap_or(b"");
                    if !matches!(ty, b"Font" | b"FontDescriptor" | b"ExtGState") {
                        continue;
                    }
                    let mut h = DefaultHasher::new();
                    format!("{:?}", d).hash(&mut h);
                    h.finish()
                }
                _ => continue,
            };
            let same = seen.entry(key).or_default();
            // A hash match is checked for real equality before merging.
            if let Some(first) = same.iter().find(|f| same_object(&objects[f], obj)) {
                remap.insert(*id, *first);
            } else {
                same.push(*id);
            }
        }
        if remap.is_empty() {
            return;
        }
        for id in remap.keys() {
            objects.remove(id);
        }
        for obj in objects.values_mut() {
            rewrite_refs(obj, &remap);
        }
    }
}

fn same_object(a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::Stream(x), Object::Stream(y)) => x.content == y.content && format!("{:?}", x.dict) == format!("{:?}", y.dict),
        (Object::Dictionary(x), Object::Dictionary(y)) => format!("{:?}", x) == format!("{:?}", y),
        _ => false,
    }
}

fn rewrite_refs(o: &mut Object, remap: &std::collections::HashMap<ObjectId, ObjectId>) {
    match o {
        Object::Reference(id) => {
            if let Some(n) = remap.get(id) {
                *id = *n;
            }
        }
        Object::Array(a) => a.iter_mut().for_each(|x| rewrite_refs(x, remap)),
        Object::Dictionary(d) => d.iter_mut().for_each(|(_, v)| rewrite_refs(v, remap)),
        Object::Stream(s) => s.dict.iter_mut().for_each(|(_, v)| rewrite_refs(v, remap)),
        _ => {}
    }
}
