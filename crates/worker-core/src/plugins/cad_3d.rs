//! 3D models and CAD drawings.
//! - Models: STL, OBJ, PLY, glTF/GLB, 3MF and FBX (ufbx) into STL, OBJ, GLB, 3MF or PLY,
//!   with units (mm, cm, m, inch) and the up axis (Z up for printing and CAD, Y up for
//!   games and the web) put right. The shape only: colours and textures stay behind.
//! - Drawings: DXF into SVG, PNG or PDF (lines, polylines with arcs, circles, arcs,
//!   ellipses, splines, text, blocks, dimensions). DWG is a closed format: save it as DXF.

use super::{file_size, write_file, Analysis, Ctx, Plugin};
use contract::{codes, opt, t, FileError, FileRef, Request};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::Path;

pub const MODELS: &[&str] = &["stl", "obj", "ply", "glb", "gltf", "3mf", "fbx"];
pub const DRAWINGS: &[&str] = &["dxf", "dwg"];

pub struct Cad3d;

impl Plugin for Cad3d {
    fn id(&self) -> &'static str {
        "cad-3d"
    }

    fn manifest(&self) -> Value {
        let model = json!({ "file": "kind", "eq": "model", "quant": "any" });
        let drawing = json!({ "file": "kind", "eq": "drawing", "quant": "any" });
        let inputs: Vec<&str> = MODELS.iter().chain(DRAWINGS).copied().collect();
        let units = || json!([ opt("auto", "Ca fișierul", "As the file says"), opt("mm", "milimetri", "millimetres"), opt("cm", "centimetri", "centimetres"), opt("m", "metri", "metres"), opt("in", "inch", "inches") ]);
        json!({
            "name": t("Modele 3D și CAD", "3D models and CAD"),
            "summary": t("STL, OBJ, GLB, 3MF, FBX, DXF", "STL, OBJ, GLB, 3MF, FBX, DXF"),
            "keywords": { "ro": ["stl", "obj", "glb", "3mf", "fbx", "dxf", "model 3d", "imprimare 3d"],
                          "en": ["stl", "obj", "glb", "3mf", "fbx", "dxf", "3d model", "3d printing"] },
            "category": "other",
            "icon": "box",
            "input": { "extensions": inputs, "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["stl", "obj", "glb", "3mf", "ply", "svg", "png", "pdf"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["kind", "triangles"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 2.0 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Unități și orientare", "Units and orientation"), "order": 1 } ],
            "fields": [
                { "key": "model", "type": "select", "label": t("Modelele 3D în", "3D models to"), "display": "chips", "showIf": model,
                  "default": [ { "when": { "file": "ext", "eq": "stl", "quant": "all" }, "value": "3mf" }, { "value": "stl" } ],
                  "options": [ opt("stl", "STL", "STL"), opt("3mf", "3MF", "3MF"), opt("obj", "OBJ", "OBJ"), opt("glb", "GLB", "GLB"), opt("ply", "PLY", "PLY") ] },
                { "key": "drawing", "type": "select", "label": t("Desenele în", "Drawings to"), "default": "pdf", "display": "chips", "showIf": drawing,
                  "options": [ opt("pdf", "PDF", "PDF"), opt("svg", "SVG", "SVG"), opt("png", "PNG", "PNG") ] },
                { "key": "unitFrom", "type": "select", "advanced": true, "group": "more", "label": t("Unitatea modelului", "The model's unit"), "default": "auto", "showIf": model,
                  "options": units(), "help": t("STL, OBJ și PLY nu-și spun unitatea; de obicei e milimetrul.", "STL, OBJ and PLY don't say their unit; it's usually millimetres.") },
                { "key": "unitTo", "type": "select", "advanced": true, "group": "more", "label": t("Unitatea rezultatului", "Unit of the result"), "default": "auto", "showIf": model,
                  "options": units() },
                { "key": "up", "type": "select", "advanced": true, "group": "more", "label": t("Axa în sus", "Up axis"), "default": "auto", "showIf": model,
                  "options": [ opt("auto", "Cum cere formatul nou (Z la STL/3MF/PLY, Y la GLB/OBJ)", "As the new format expects (Z for STL/3MF/PLY, Y for GLB/OBJ)"), opt("keep", "Nu roti", "Don't rotate") ] }
            ],
            "rules": [
                { "id": "shape_only", "severity": "info", "when": { "all": [ model, { "field": "model", "in": ["stl", "obj", "3mf", "ply", "glb"] } ] },
                  "message": t("Se păstrează forma; culorile și texturile nu.", "The shape is kept; colours and textures are not.") },
                { "id": "dwg", "severity": "error", "when": { "file": "ext", "eq": "dwg", "quant": "any" },
                  "message": t("DWG este un format închis. Salvează desenul ca DXF din programul de CAD (aproape toate pot), apoi convertește-l aici.",
                               "DWG is a closed format. Save the drawing as DXF from your CAD program (almost all can), then convert it here.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}", "{name}") } },
            "errors": {
                "model_unreadable": t("Modelul nu poate fi citit: {message}", "The model can't be read: {message}"),
                "drawing_unreadable": t("Desenul nu poate fi citit: {message}", "The drawing can't be read: {message}"),
                "dwg": t("DWG este un format închis; salvează desenul ca DXF.", "DWG is a closed format; save the drawing as DXF."),
                "empty": t("Fișierul nu are nimic de desenat.", "The file has nothing to draw.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, _req: &Request) -> Analysis {
        let ext = ext_of(f.input_path());
        let mut p = Map::new();
        p.insert("ext".into(), json!(ext));
        p.insert("kind".into(), json!(if DRAWINGS.contains(&ext.as_str()) { "drawing" } else { "model" }));
        if ext == "dwg" {
            return Analysis { status: "unsupported", props: p, warnings: vec![], code: Some("dwg".into()) };
        }
        Analysis::ok(p)
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let path = f.input_path();
        let in_bytes = file_size(path);
        let ext = ext_of(path);
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let dir = ctx.file_dir(&f.id)?;
        let (dest, out_ext) = match ext.as_str() {
            "dwg" => return Err(FileError::new("dwg")),
            "dxf" => {
                let format = ctx.req.str_or("drawing", "pdf");
                let svg = dxf_svg(path)?;
                ctx.progress(&f.id, 0.5, "write", 2, 2);
                let dest = dir.join(format!("result.{format}"));
                match format {
                    "svg" => write_file(&dest, svg.as_bytes())?,
                    _ => {
                        let opt = resvg::usvg::Options { fontdb: crate::common::font::system(), ..Default::default() };
                        let tree = resvg::usvg::Tree::from_str(&svg, &opt).map_err(|e| FileError::with("drawing_unreadable", json!({ "message": e.to_string() })))?;
                        if format == "png" {
                            let (w, h) = (tree.size().width(), tree.size().height());
                            let k = 3000.0 / w.max(h);
                            let (pw, ph) = ((w * k).round().max(1.0) as u32, (h * k).round().max(1.0) as u32);
                            let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph).ok_or_else(|| FileError::new(codes::CORRUPT_FILE))?;
                            pixmap.fill(resvg::tiny_skia::Color::WHITE);
                            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
                            write_file(&dest, &pixmap.encode_png().map_err(FileError::engine)?)?;
                        } else {
                            super::svg::svg_pdf(ctx, f, &tree, &dest)?;
                        }
                    }
                }
                (dest, format)
            }
            _ => {
                let format = ctx.req.str_or("model", "stl");
                let (mut mesh, src) = read_model(path, &ext)?;
                if mesh.tri.is_empty() {
                    return Err(FileError::new("empty"));
                }
                ctx.check_cancel()?;
                ctx.progress(&f.id, 0.5, "write", 2, 2);
                // Units.
                let from = unit_m(ctx.req.str_or("unitFrom", "auto")).unwrap_or(src.unit_m);
                // glTF is in metres by definition; printers and CAD read STL, OBJ, PLY
                // and 3MF in millimetres.
                let to = unit_m(ctx.req.str_or("unitTo", "auto")).unwrap_or(if format == "glb" { 1.0 } else { 0.001 });
                let k = (from / to) as f32;
                // Up axis.
                let want_y = matches!(format, "glb" | "obj");
                let rot = if ctx.req.str_or("up", "auto") == "keep" { 0 } else if src.y_up && !want_y { 1 } else if !src.y_up && want_y { -1 } else { 0 };
                for p in &mut mesh.pos {
                    let [x, y, z] = *p;
                    *p = match rot {
                        1 => [x * k, -z * k, y * k],
                        -1 => [x * k, z * k, -y * k],
                        _ => [x * k, y * k, z * k],
                    };
                }
                let bytes = match format {
                    "obj" => write_obj(&mesh),
                    "ply" => write_ply(&mesh),
                    "glb" => write_glb(&mesh),
                    "3mf" => write_3mf(&mesh, unit_name(to))?,
                    _ => write_stl(&mesh),
                };
                let dest = dir.join(format!("result.{format}"));
                write_file(&dest, &bytes)?;
                (dest, format)
            }
        };
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        ctx.emit_output(&f.id, &dest, out_ext, json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn unit_m(u: &str) -> Option<f64> {
    match u {
        "mm" => Some(0.001),
        "cm" => Some(0.01),
        "m" => Some(1.0),
        "in" => Some(0.0254),
        _ => None,
    }
}

fn unit_name(m: f64) -> &'static str {
    if (m - 0.01).abs() < 1e-9 {
        "centimeter"
    } else if (m - 1.0).abs() < 1e-9 {
        "meter"
    } else if (m - 0.0254).abs() < 1e-9 {
        "inch"
    } else {
        "millimeter"
    }
}

fn bad(msg: impl std::fmt::Display) -> FileError {
    FileError::with("model_unreadable", json!({ "message": msg.to_string() }))
}

// ------------------------------------------------------------------ models

#[derive(Default)]
struct Mesh {
    pos: Vec<[f32; 3]>,
    tri: Vec<[u32; 3]>,
}

impl Mesh {
    /// Adds a triangle by its corners, sharing equal corners (STL repeats them).
    fn weld(&mut self, map: &mut HashMap<[u32; 3], u32>, corners: [[f32; 3]; 3]) {
        let mut idx = [0u32; 3];
        for (k, c) in corners.iter().enumerate() {
            let key = [c[0].to_bits(), c[1].to_bits(), c[2].to_bits()];
            idx[k] = *map.entry(key).or_insert_with(|| {
                self.pos.push(*c);
                (self.pos.len() - 1) as u32
            });
        }
        if idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
            self.tri.push(idx);
        }
    }
}

/// What the source says about itself.
struct Source {
    unit_m: f64,
    y_up: bool,
}

fn read_model(path: &Path, ext: &str) -> Result<(Mesh, Source), FileError> {
    let mm_z = Source { unit_m: 0.001, y_up: false };
    match ext {
        "stl" => Ok((read_stl(&std::fs::read(path).map_err(super::io_err)?)?, mm_z)),
        "obj" => Ok((read_obj(&std::fs::read(path).map_err(super::io_err)?), Source { unit_m: 0.001, y_up: true })),
        "ply" => Ok((read_ply(&std::fs::read(path).map_err(super::io_err)?)?, mm_z)),
        "3mf" => read_3mf(path),
        "fbx" => read_fbx(path),
        _ => read_gltf(path),
    }
}

fn read_stl(d: &[u8]) -> Result<Mesh, FileError> {
    let mut m = Mesh::default();
    let mut map = HashMap::new();
    let n = d.get(80..84).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).unwrap_or(0);
    if d.len() >= 84 && d.len() == 84 + n * 50 {
        for t in d[84..].chunks_exact(50) {
            let v = |o: usize| [f32::from_le_bytes(t[o..o + 4].try_into().unwrap()), f32::from_le_bytes(t[o + 4..o + 8].try_into().unwrap()), f32::from_le_bytes(t[o + 8..o + 12].try_into().unwrap())];
            m.weld(&mut map, [v(12), v(24), v(36)]);
        }
        return Ok(m);
    }
    let text = String::from_utf8_lossy(d);
    let mut corners = Vec::with_capacity(3);
    for line in text.lines() {
        let mut w = line.split_whitespace();
        if w.next() == Some("vertex") {
            let v: Vec<f32> = w.take(3).filter_map(|x| x.parse().ok()).collect();
            if v.len() == 3 {
                corners.push([v[0], v[1], v[2]]);
            }
            if corners.len() == 3 {
                m.weld(&mut map, [corners[0], corners[1], corners[2]]);
                corners.clear();
            }
        } else if line.trim_start().starts_with("endloop") {
            corners.clear();
        }
    }
    if m.tri.is_empty() && !text.trim_start().starts_with("solid") {
        return Err(bad("not an STL file"));
    }
    Ok(m)
}

fn read_obj(d: &[u8]) -> Mesh {
    let text = String::from_utf8_lossy(d);
    let mut m = Mesh::default();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => {
                let v: Vec<f32> = w.take(3).filter_map(|x| x.parse().ok()).collect();
                if v.len() == 3 {
                    m.pos.push([v[0], v[1], v[2]]);
                }
            }
            Some("f") => {
                let n = m.pos.len() as i64;
                let idx: Vec<u32> = w.filter_map(|c| c.split('/').next()?.parse::<i64>().ok()).map(|i| if i < 0 { n + i } else { i - 1 }).filter(|&i| i >= 0 && i < n).map(|i| i as u32).collect();
                for k in 1..idx.len().saturating_sub(1) {
                    m.tri.push([idx[0], idx[k], idx[k + 1]]);
                }
            }
            _ => {}
        }
    }
    m
}

fn read_ply(d: &[u8]) -> Result<Mesh, FileError> {
    let end = d.windows(10).position(|w| w == b"end_header").ok_or_else(|| bad("not a PLY file"))?;
    let header = String::from_utf8_lossy(&d[..end]);
    let mut body = end + 10;
    while body < d.len() && (d[body] == b'\r' || d[body] == b'\n') {
        body += 1;
        if d[body - 1] == b'\n' {
            break;
        }
    }
    let format = header.lines().find(|l| l.starts_with("format")).unwrap_or("format ascii").split_whitespace().nth(1).unwrap_or("ascii").to_string();
    // Elements with their properties: (name, count, [(type, name, list count type)]).
    let mut elements: Vec<(String, usize, Vec<(String, String, Option<String>)>)> = Vec::new();
    for l in header.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.first() {
            Some(&"element") if w.len() >= 3 => elements.push((w[1].into(), w[2].parse().unwrap_or(0), Vec::new())),
            Some(&"property") if w.len() >= 3 => {
                if let Some(e) = elements.last_mut() {
                    if w[1] == "list" && w.len() >= 5 {
                        e.2.push((w[3].into(), w[4].into(), Some(w[2].into())));
                    } else {
                        e.2.push((w[1].into(), w[2].into(), None));
                    }
                }
            }
            _ => {}
        }
    }
    let size = |t: &str| match t {
        "char" | "uchar" | "int8" | "uint8" => 1,
        "short" | "ushort" | "int16" | "uint16" => 2,
        "double" | "float64" => 8,
        _ => 4,
    };
    let mut m = Mesh::default();
    if format == "ascii" {
        let text = String::from_utf8_lossy(&d[body..]);
        let mut nums = text.split_whitespace().map(|x| x.parse::<f64>().unwrap_or(0.0));
        for (name, count, props) in &elements {
            for _ in 0..*count {
                let mut xyz = [0f32; 3];
                for (_, pname, list) in props {
                    if list.is_some() {
                        let n = nums.next().unwrap_or(0.0) as usize;
                        let idx: Vec<u32> = (0..n).map(|_| nums.next().unwrap_or(0.0) as u32).collect();
                        if name == "face" {
                            for k in 1..idx.len().saturating_sub(1) {
                                m.tri.push([idx[0], idx[k], idx[k + 1]]);
                            }
                        }
                    } else {
                        let v = nums.next().unwrap_or(0.0) as f32;
                        match pname.as_str() {
                            "x" => xyz[0] = v,
                            "y" => xyz[1] = v,
                            "z" => xyz[2] = v,
                            _ => {}
                        }
                    }
                }
                if name == "vertex" {
                    m.pos.push(xyz);
                }
            }
        }
    } else {
        let be = format.contains("big");
        let mut at = body;
        let read = |t: &str, at: &mut usize| -> f64 {
            let n = size(t);
            let b = d.get(*at..*at + n).unwrap_or(&[0; 8][..n]);
            *at += n;
            macro_rules! num {
                ($ty:ty) => {{
                    let a: [u8; std::mem::size_of::<$ty>()] = b.try_into().unwrap();
                    (if be { <$ty>::from_be_bytes(a) } else { <$ty>::from_le_bytes(a) }) as f64
                }};
            }
            match t {
                "char" | "int8" => b[0] as i8 as f64,
                "uchar" | "uint8" => b[0] as f64,
                "short" | "int16" => num!(i16),
                "ushort" | "uint16" => num!(u16),
                "int" | "int32" => num!(i32),
                "uint" | "uint32" => num!(u32),
                "double" | "float64" => num!(f64),
                _ => num!(f32),
            }
        };
        for (name, count, props) in &elements {
            for _ in 0..*count {
                let mut xyz = [0f32; 3];
                for (t, pname, list) in props {
                    if let Some(ct) = list {
                        let n = read(ct, &mut at) as usize;
                        let idx: Vec<u32> = (0..n).map(|_| read(t, &mut at) as u32).collect();
                        if name == "face" {
                            for k in 1..idx.len().saturating_sub(1) {
                                m.tri.push([idx[0], idx[k], idx[k + 1]]);
                            }
                        }
                    } else {
                        let v = read(t, &mut at) as f32;
                        match pname.as_str() {
                            "x" => xyz[0] = v,
                            "y" => xyz[1] = v,
                            "z" => xyz[2] = v,
                            _ => {}
                        }
                    }
                }
                if name == "vertex" {
                    m.pos.push(xyz);
                }
            }
        }
    }
    let n = m.pos.len() as u32;
    m.tri.retain(|t| t.iter().all(|&i| i < n));
    Ok(m)
}

fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Column-major, as glTF stores them.
    let mut r = [[0f32; 4]; 4];
    for (c, col) in r.iter_mut().enumerate() {
        for (row, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][row] * b[c][k]).sum();
        }
    }
    r
}

fn apply(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

fn read_gltf(path: &Path) -> Result<(Mesh, Source), FileError> {
    let (doc, buffers, _) = gltf::import(path).map_err(bad)?;
    let mut m = Mesh::default();
    fn walk(node: gltf::Node, parent: [[f32; 4]; 4], buffers: &[gltf::buffer::Data], m: &mut Mesh) {
        let world = mat_mul(&parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives().filter(|p| p.mode() == gltf::mesh::Mode::Triangles) {
                let reader = prim.reader(|b| Some(&buffers[b.index()]));
                let Some(positions) = reader.read_positions() else { continue };
                let base = m.pos.len() as u32;
                m.pos.extend(positions.map(|p| apply(&world, p)));
                let count = m.pos.len() as u32 - base;
                match reader.read_indices() {
                    Some(ix) => {
                        let ix: Vec<u32> = ix.into_u32().collect();
                        m.tri.extend(ix.chunks_exact(3).map(|t| [base + t[0], base + t[1], base + t[2]]));
                    }
                    None => m.tri.extend((0..count / 3).map(|i| [base + i * 3, base + i * 3 + 1, base + i * 3 + 2])),
                }
            }
        }
        for c in node.children() {
            walk(c, world, buffers, m);
        }
    }
    let id = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or_else(|| bad("no scene"))?;
    for n in scene.nodes() {
        walk(n, id, &buffers, &mut m);
    }
    Ok((m, Source { unit_m: 1.0, y_up: true }))
}

fn read_fbx(path: &Path) -> Result<(Mesh, Source), FileError> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        ..Default::default()
    };
    let scene = ufbx::load_file(&path.to_string_lossy(), opts).map_err(|e| bad(e.description.to_string()))?;
    let mut m = Mesh::default();
    let mut idx = Vec::new();
    for node in scene.nodes.iter() {
        let Some(mesh) = node.mesh.as_ref() else { continue };
        let base = m.pos.len() as u32;
        for i in 0..mesh.num_indices {
            let p = ufbx::transform_position(&node.geometry_to_world, mesh.vertex_position[i]);
            m.pos.push([p.x as f32, p.y as f32, p.z as f32]);
        }
        for face in mesh.faces.iter() {
            idx.resize((mesh.max_face_triangles * 3).max(3), 0);
            let n = mesh.triangulate_face(&mut idx, *face) as usize;
            for t in idx[..n * 3].chunks_exact(3) {
                m.tri.push([base + t[0], base + t[1], base + t[2]]);
            }
        }
    }
    Ok((m, Source { unit_m: 1.0, y_up: true }))
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let p = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    tag[p..].find('"').map(|e| tag[p..p + e].to_string())
}

fn read_3mf(path: &Path) -> Result<(Mesh, Source), FileError> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).map_err(super::io_err)?).map_err(bad)?;
    let name = (0..zip.len()).filter_map(|i| zip.by_index(i).ok().and_then(|e| e.name().ok().map(|n| n.to_string()))).find(|n| n.to_ascii_lowercase().ends_with(".model")).ok_or_else(|| bad("no model inside"))?;
    let mut xml = String::new();
    zip.by_name(&name).map_err(bad)?.read_to_string(&mut xml).map_err(super::io_err)?;
    let unit = xml.find("<model").and_then(|s| xml_attr(&xml[s..s + xml[s..].find('>').unwrap_or(0)], "unit")).unwrap_or_else(|| "millimeter".into());
    let unit_m = match unit.as_str() {
        "micron" => 1e-6,
        "centimeter" => 0.01,
        "inch" => 0.0254,
        "foot" => 0.3048,
        "meter" => 1.0,
        _ => 0.001,
    };
    // Objects: id → (vertices, triangles, components).
    type Object = (Vec<[f32; 3]>, Vec<[u32; 3]>, Vec<(String, Option<String>)>);
    let mut objects: HashMap<String, Object> = HashMap::new();
    let mut at = 0;
    while let Some(s) = xml[at..].find("<object ").map(|i| at + i) {
        let e = xml[s..].find("</object>").map(|i| s + i).unwrap_or(xml.len());
        let body = &xml[s..e];
        let id = xml_attr(body, "id").unwrap_or_default();
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        let mut comps = Vec::new();
        for tag in body.split('<').skip(1) {
            if tag.starts_with("vertex ") {
                let g = |k| xml_attr(tag, k).and_then(|v| v.parse().ok()).unwrap_or(0.0f32);
                verts.push([g("x"), g("y"), g("z")]);
            } else if tag.starts_with("triangle ") {
                let g = |k| xml_attr(tag, k).and_then(|v| v.parse().ok()).unwrap_or(0u32);
                tris.push([g("v1"), g("v2"), g("v3")]);
            } else if tag.starts_with("component ") {
                comps.push((xml_attr(tag, "objectid").unwrap_or_default(), xml_attr(tag, "transform")));
            }
        }
        objects.insert(id, (verts, tris, comps));
        at = e;
    }
    let parse_t = |t: Option<String>| -> [[f32; 4]; 4] {
        let v: Vec<f32> = t.unwrap_or_default().split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() == 12 {
            // 3MF: m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32, row vectors.
            [[v[0], v[1], v[2], 0.0], [v[3], v[4], v[5], 0.0], [v[6], v[7], v[8], 0.0], [v[9], v[10], v[11], 1.0]]
        } else {
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
        }
    };
    let mut m = Mesh::default();
    fn add(objects: &HashMap<String, Object>, id: &str, t: [[f32; 4]; 4], m: &mut Mesh, depth: u32, parse_t: &dyn Fn(Option<String>) -> [[f32; 4]; 4]) {
        let Some((v, tr, comps)) = objects.get(id) else { return };
        let base = m.pos.len() as u32;
        m.pos.extend(v.iter().map(|p| apply(&t, *p)));
        m.tri.extend(tr.iter().filter(|x| x.iter().all(|&i| (i as usize) < v.len())).map(|x| [base + x[0], base + x[1], base + x[2]]));
        if depth < 8 {
            for (cid, ct) in comps {
                add(objects, cid, mat_mul(&t, &parse_t(ct.clone())), m, depth + 1, parse_t);
            }
        }
    }
    let build = xml.find("<build").map(|s| &xml[s..]).unwrap_or("");
    let items: Vec<(String, Option<String>)> = build.split("<item ").skip(1).map(|t| (xml_attr(&format!(" {t}"), "objectid").unwrap_or_default(), xml_attr(&format!(" {t}"), "transform"))).collect();
    if items.is_empty() {
        for id in objects.keys() {
            add(&objects, id, parse_t(None), &mut m, 0, &parse_t);
        }
    }
    for (id, tr) in items {
        add(&objects, &id, parse_t(tr), &mut m, 0, &parse_t);
    }
    Ok((m, Source { unit_m, y_up: false }))
}

fn normal(m: &Mesh, t: &[u32; 3]) -> [f32; 3] {
    let (a, b, c) = (m.pos[t[0] as usize], m.pos[t[1] as usize], m.pos[t[2] as usize]);
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-20);
    [n[0] / l, n[1] / l, n[2] / l]
}

fn write_stl(m: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + m.tri.len() * 50);
    let mut header = [0u8; 80];
    let tag = b"MQKit binary STL";
    header[..tag.len()].copy_from_slice(tag);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(m.tri.len() as u32).to_le_bytes());
    for t in &m.tri {
        for v in normal(m, t).iter().chain(t.iter().flat_map(|&i| m.pos[i as usize].iter())) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

fn write_obj(m: &Mesh) -> Vec<u8> {
    let mut s = String::from("# MQKit\n");
    for p in &m.pos {
        let _ = writeln!(s, "v {} {} {}", p[0], p[1], p[2]);
    }
    for t in &m.tri {
        let _ = writeln!(s, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
    }
    s.into_bytes()
}

fn write_ply(m: &Mesh) -> Vec<u8> {
    let mut out = format!("ply\nformat binary_little_endian 1.0\ncomment MQKit\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nelement face {}\nproperty list uchar uint vertex_indices\nend_header\n", m.pos.len(), m.tri.len()).into_bytes();
    for p in &m.pos {
        for v in p {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    for t in &m.tri {
        out.push(3);
        for i in t {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    out
}

/// glTF binary: positions and indices (flat shading: no normals, as glTF allows).
fn write_glb(m: &Mesh) -> Vec<u8> {
    let mut bin = Vec::with_capacity(m.pos.len() * 12 + m.tri.len() * 12);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &m.pos {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
            bin.extend_from_slice(&p[k].to_le_bytes());
        }
    }
    let pos_len = bin.len();
    for t in &m.tri {
        for i in t {
            bin.extend_from_slice(&i.to_le_bytes());
        }
    }
    let idx_len = bin.len() - pos_len;
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let gltf = json!({
        "asset": { "version": "2.0", "generator": "MQKit" },
        "scene": 0, "scenes": [ { "nodes": [0] } ], "nodes": [ { "mesh": 0 } ],
        "meshes": [ { "primitives": [ { "attributes": { "POSITION": 0 }, "indices": 1, "material": 0 } ] } ],
        "materials": [ { "pbrMetallicRoughness": { "baseColorFactor": [0.8, 0.8, 0.8, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.8 } } ],
        "buffers": [ { "byteLength": bin.len() } ],
        "bufferViews": [ { "buffer": 0, "byteOffset": 0, "byteLength": pos_len, "target": 34962 }, { "buffer": 0, "byteOffset": pos_len, "byteLength": idx_len, "target": 34963 } ],
        "accessors": [ { "bufferView": 0, "componentType": 5126, "count": m.pos.len(), "type": "VEC3", "min": lo, "max": hi },
                       { "bufferView": 1, "componentType": 5125, "count": m.tri.len() * 3, "type": "SCALAR" } ]
    });
    let mut js = serde_json::to_vec(&gltf).unwrap_or_default();
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&js);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn write_3mf(m: &Mesh, unit: &str) -> Result<Vec<u8>, FileError> {
    let mut model = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"{unit}\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources><object id=\"1\" type=\"model\"><mesh><vertices>");
    for p in &m.pos {
        let _ = write!(model, "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", p[0], p[1], p[2]);
    }
    model.push_str("</vertices><triangles>");
    for t in &m.tri {
        let _ = write!(model, "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]);
    }
    model.push_str("</triangles></mesh></object></resources><build><item objectid=\"1\"/></build></model>");
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let files = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#.to_string()),
        ("3D/3dmodel.model", model),
    ];
    for (n, body) in files {
        z.start_file(n, o).map_err(FileError::engine)?;
        z.write_all(body.as_bytes()).map_err(super::io_err)?;
    }
    Ok(z.finish().map_err(FileError::engine)?.into_inner())
}

// ------------------------------------------------------------------ DXF drawings

/// 2D affine transform: (a, b, c, d, e, f) as in SVG's matrix().
#[derive(Clone, Copy)]
struct Tf([f64; 6]);

impl Tf {
    const ID: Tf = Tf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn then(self, o: Tf) -> Tf {
        // self applied first, then o.
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        Tf([a2 * a + c2 * b, b2 * a + d2 * b, a2 * c + c2 * d, b2 * c + d2 * d, a2 * e + c2 * f + e2, b2 * e + d2 * f + f2])
    }
    fn p(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }
    fn scale(&self) -> f64 {
        let [a, b, c, d, _, _] = self.0;
        ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0
    }
}

/// AutoCAD's colour index, darkened where it would vanish on white paper.
fn aci(i: u8) -> &'static str {
    match i {
        1 => "#e00000",
        2 => "#b89a00",
        3 => "#00a000",
        4 => "#0097a7",
        5 => "#0030e0",
        6 => "#c000c0",
        8 => "#808080",
        9 => "#a0a0a0",
        _ => "#000000",
    }
}

struct Svg {
    out: String,
    lo: (f64, f64),
    hi: (f64, f64),
}

impl Svg {
    fn grow(&mut self, p: (f64, f64)) {
        self.lo = (self.lo.0.min(p.0), self.lo.1.min(p.1));
        self.hi = (self.hi.0.max(p.0), self.hi.1.max(p.1));
    }
    fn poly(&mut self, pts: &[(f64, f64)], closed: bool, color: &str, fill: bool) {
        if pts.len() < 2 {
            return;
        }
        let mut d = String::new();
        for (k, p) in pts.iter().enumerate() {
            self.grow(*p);
            let _ = write!(d, "{}{:.4} {:.4}", if k == 0 { "M" } else { " L" }, p.0, p.1);
        }
        if closed {
            d.push_str(" Z");
        }
        let fill = if fill { color } else { "none" };
        let _ = writeln!(self.out, "<path d=\"{d}\" stroke=\"{color}\" fill=\"{fill}\"/>");
    }
}

fn arc_points(cx: f64, cy: f64, r: f64, a0: f64, a1: f64) -> Vec<(f64, f64)> {
    let mut sweep = a1 - a0;
    while sweep <= 0.0 {
        sweep += std::f64::consts::TAU;
    }
    let n = ((sweep / std::f64::consts::TAU) * 96.0).ceil().max(4.0) as usize;
    (0..=n).map(|i| {
        let a = a0 + sweep * i as f64 / n as f64;
        (cx + r * a.cos(), cy + r * a.sin())
    }).collect()
}

/// Polyline vertices with bulges (arc segments) into points.
fn bulge_points(v: &[(f64, f64, f64)], closed: bool) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    let n = v.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let (x0, y0, b) = v[i];
        let (x1, y1, _) = v[(i + 1) % n];
        pts.push((x0, y0));
        if b.abs() > 1e-9 {
            let angle = 4.0 * b.atan();
            let chord = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            if chord < 1e-12 {
                continue;
            }
            let r = chord / (2.0 * (angle / 2.0).sin());
            let mid = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let h = r * (angle / 2.0).cos();
            let (nx, ny) = (-(y1 - y0) / chord, (x1 - x0) / chord);
            let c = (mid.0 + nx * h, mid.1 + ny * h);
            let a0 = (y0 - c.1).atan2(x0 - c.0);
            let steps = ((angle.abs() / std::f64::consts::TAU) * 96.0).ceil().max(2.0) as usize;
            for k in 1..steps {
                let a = a0 + angle * k as f64 / steps as f64;
                pts.push((c.0 + r.abs() * a.cos(), c.1 + r.abs() * a.sin()));
            }
        }
    }
    if !closed {
        if let Some(&(x, y, _)) = v.last() {
            pts.push((x, y));
        }
    }
    pts
}

/// A B-spline from its control points and knots (de Boor), else its fit points.
fn spline_points(s: &dxf::entities::Spline) -> Vec<(f64, f64)> {
    let cps: Vec<(f64, f64)> = s.control_points.iter().map(|p| (p.x, p.y)).collect();
    let k = s.degree_of_curve.max(1) as usize;
    let knots = &s.knot_values;
    if cps.len() > k && knots.len() == cps.len() + k + 1 {
        let (t0, t1) = (knots[k], knots[cps.len()]);
        let n = (cps.len() * 16).max(32);
        return (0..=n).map(|i| {
            let t = t0 + (t1 - t0) * i as f64 / n as f64;
            let mut span = k;
            while span < cps.len() - 1 && t >= knots[span + 1] {
                span += 1;
            }
            let mut d: Vec<(f64, f64)> = (0..=k).map(|j| cps[span - k + j]).collect();
            for r in 1..=k {
                for j in (r..=k).rev() {
                    let i = span - k + j;
                    let den = knots[i + k + 1 - r] - knots[i];
                    let a = if den.abs() < 1e-12 { 0.0 } else { (t - knots[i]) / den };
                    d[j] = (d[j - 1].0 * (1.0 - a) + d[j].0 * a, d[j - 1].1 * (1.0 - a) + d[j].1 * a);
                }
            }
            d[k]
        }).collect();
    }
    if !s.fit_points.is_empty() {
        return s.fit_points.iter().map(|p| (p.x, p.y)).collect();
    }
    cps
}

fn draw(dw: &dxf::Drawing, entities: &mut dyn Iterator<Item = &dxf::entities::Entity>, tf: Tf, layer_colors: &HashMap<String, u8>, parent_color: u8, svg: &mut Svg, depth: u32) {
    use dxf::entities::EntityType as E;
    for e in entities {
        let c = &e.common;
        if !c.is_visible {
            continue;
        }
        let ci = if c.color.is_by_layer() { layer_colors.get(&c.layer).copied().unwrap_or(7) } else if c.color.is_by_block() { parent_color } else { c.color.index().unwrap_or(7) };
        let color = aci(ci);
        let tp = |x: f64, y: f64| tf.p(x, y);
        match &e.specific {
            E::Line(l) => svg.poly(&[tp(l.p1.x, l.p1.y), tp(l.p2.x, l.p2.y)], false, color, false),
            E::Circle(ci) => {
                let pts: Vec<_> = arc_points(ci.center.x, ci.center.y, ci.radius, 0.0, std::f64::consts::TAU).into_iter().map(|(x, y)| tp(x, y)).collect();
                svg.poly(&pts, true, color, false);
            }
            E::Arc(a) => {
                let pts: Vec<_> = arc_points(a.center.x, a.center.y, a.radius, a.start_angle.to_radians(), a.end_angle.to_radians()).into_iter().map(|(x, y)| tp(x, y)).collect();
                svg.poly(&pts, false, color, false);
            }
            E::Ellipse(el) => {
                let (mx, my) = (el.major_axis.x, el.major_axis.y);
                let r = (mx * mx + my * my).sqrt();
                let rot = my.atan2(mx);
                let mut t1 = el.end_parameter;
                while t1 <= el.start_parameter {
                    t1 += std::f64::consts::TAU;
                }
                let n = 96;
                let pts: Vec<_> = (0..=n).map(|i| {
                    let t = el.start_parameter + (t1 - el.start_parameter) * i as f64 / n as f64;
                    let (x, y) = (r * t.cos(), r * el.minor_axis_ratio * t.sin());
                    tp(el.center.x + x * rot.cos() - y * rot.sin(), el.center.y + x * rot.sin() + y * rot.cos())
                }).collect();
                svg.poly(&pts, false, color, false);
            }
            E::LwPolyline(p) => {
                let v: Vec<_> = p.vertices.iter().map(|v| (v.x, v.y, v.bulge)).collect();
                let pts: Vec<_> = bulge_points(&v, p.is_closed()).into_iter().map(|(x, y)| tp(x, y)).collect();
                svg.poly(&pts, p.is_closed(), color, false);
            }
            E::Polyline(p) => {
                let v: Vec<_> = p.vertices().map(|v| (v.location.x, v.location.y, v.bulge)).collect();
                let pts: Vec<_> = bulge_points(&v, p.is_closed()).into_iter().map(|(x, y)| tp(x, y)).collect();
                svg.poly(&pts, p.is_closed(), color, false);
            }
            E::Spline(s) => {
                let pts: Vec<_> = spline_points(s).into_iter().map(|(x, y)| tp(x, y)).collect();
                svg.poly(&pts, s.is_closed(), color, false);
            }
            E::Solid(s) => {
                // DXF solids list their corners as 1, 2, 4, 3.
                let pts = [tp(s.first_corner.x, s.first_corner.y), tp(s.second_corner.x, s.second_corner.y), tp(s.fourth_corner.x, s.fourth_corner.y), tp(s.third_corner.x, s.third_corner.y)];
                svg.poly(&pts, true, color, true);
            }
            E::Text(tx) => text(svg, tf, tx.location.x, tx.location.y, tx.text_height, tx.rotation, &tx.value, color),
            E::MText(mt) => {
                let mut s = mt.extended_text.concat();
                s.push_str(&mt.text);
                let plain = mtext_plain(&s);
                let rot = mt.x_axis_direction.y.atan2(mt.x_axis_direction.x).to_degrees();
                for (i, line) in plain.split('\n').enumerate() {
                    let dy = mt.initial_text_height * 1.6 * i as f64;
                    let (sx, sy) = (-dy * rot.to_radians().sin(), -dy * rot.to_radians().cos());
                    text(svg, tf, mt.insertion_point.x + sx, mt.insertion_point.y + sy - mt.initial_text_height, mt.initial_text_height, rot, line, color);
                }
            }
            E::Insert(ins) => {
                if depth > 16 {
                    continue;
                }
                let Some(block) = dw.blocks().find(|b| b.name.eq_ignore_ascii_case(&ins.name)) else { continue };
                let (cos, sin) = (ins.rotation.to_radians().cos(), ins.rotation.to_radians().sin());
                for row in 0..ins.row_count.max(1) {
                    for col in 0..ins.column_count.max(1) {
                        let local = Tf([1.0, 0.0, 0.0, 1.0, -block.base_point.x, -block.base_point.y])
                            .then(Tf([ins.x_scale_factor, 0.0, 0.0, ins.y_scale_factor, 0.0, 0.0]))
                            .then(Tf([1.0, 0.0, 0.0, 1.0, col as f64 * ins.column_spacing, row as f64 * ins.row_spacing]))
                            .then(Tf([cos, sin, -sin, cos, ins.location.x, ins.location.y]))
                            .then(tf);
                        draw(dw, &mut block.entities.iter(), local, layer_colors, ci, svg, depth + 1);
                    }
                }
            }
            E::RotatedDimension(d) => dim_block(dw, &d.dimension_base.block_name, tf, layer_colors, ci, svg, depth),
            E::RadialDimension(d) => dim_block(dw, &d.dimension_base.block_name, tf, layer_colors, ci, svg, depth),
            E::DiameterDimension(d) => dim_block(dw, &d.dimension_base.block_name, tf, layer_colors, ci, svg, depth),
            E::AngularThreePointDimension(d) => dim_block(dw, &d.dimension_base.block_name, tf, layer_colors, ci, svg, depth),
            E::OrdinateDimension(d) => dim_block(dw, &d.dimension_base.block_name, tf, layer_colors, ci, svg, depth),
            _ => {}
        }
    }
}

/// A dimension is drawn by its own anonymous block ("*D…").
fn dim_block(dw: &dxf::Drawing, name: &str, tf: Tf, layer_colors: &HashMap<String, u8>, ci: u8, svg: &mut Svg, depth: u32) {
    if depth > 16 {
        return;
    }
    if let Some(b) = dw.blocks().find(|b| b.name.eq_ignore_ascii_case(name)) {
        draw(dw, &mut b.entities.iter(), tf, layer_colors, ci, svg, depth + 1);
    }
}

/// MTEXT without its formatting codes (\P new paragraph, {\fArial|…; …}, \H2.5; …).
fn mtext_plain(s: &str) -> String {
    let mut out = String::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' if i + 1 < c.len() => {
                let k = c[i + 1];
                match k {
                    'P' | 'X' => {
                        out.push('\n');
                        i += 2;
                    }
                    '\\' | '{' | '}' => {
                        out.push(k);
                        i += 2;
                    }
                    '~' => {
                        out.push(' ');
                        i += 2;
                    }
                    'L' | 'l' | 'O' | 'o' | 'K' | 'k' => i += 2,
                    'U' if c.get(i + 2) == Some(&'+') => {
                        let hex: String = c[i + 3..(i + 7).min(c.len())].iter().collect();
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            out.push(ch);
                        }
                        i += 7;
                    }
                    _ => {
                        // \f…; \H…; \C…; \A…; \W…; \T…; \Q…; \S…;
                        while i < c.len() && c[i] != ';' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
            }
            '{' | '}' => i += 1,
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn text(svg: &mut Svg, tf: Tf, x: f64, y: f64, h: f64, rot: f64, s: &str, color: &str) {
    let s = s.replace("%%c", "Ø").replace("%%d", "°").replace("%%p", "±").replace("%%C", "Ø").replace("%%D", "°").replace("%%P", "±");
    if s.trim().is_empty() {
        return;
    }
    let (px, py) = tf.p(x, y);
    let size = h * tf.scale();
    let [a, b, ..] = tf.0;
    let angle = rot + b.atan2(a).to_degrees();
    // The text's box (width guessed from the letters), all four corners.
    let (w, (ca, sa)) = (size * 0.62 * s.chars().count() as f64, (angle.to_radians().cos(), angle.to_radians().sin()));
    for (u, v) in [(0.0, 0.0), (w, 0.0), (0.0, size), (w, size)] {
        svg.grow((px + u * ca - v * sa, py + u * sa + v * ca));
    }
    let esc = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    // The drawing is upside down (y up); text is turned back so it reads right.
    let _ = writeln!(svg.out, "<text transform=\"translate({px:.4} {py:.4}) rotate({angle:.3}) scale(1 -1)\" font-size=\"{size:.4}\" fill=\"{color}\" stroke=\"none\" font-family=\"Segoe UI, Arial, sans-serif\">{esc}</text>");
}

fn dxf_svg(path: &Path) -> Result<String, FileError> {
    let dw = dxf::Drawing::load_file(path).map_err(|e| FileError::with("drawing_unreadable", json!({ "message": e.to_string() })))?;
    let layer_colors: HashMap<String, u8> = dw.layers().map(|l| (l.name.clone(), l.color.index().unwrap_or(7))).collect();
    let mut svg = Svg { out: String::new(), lo: (f64::MAX, f64::MAX), hi: (f64::MIN, f64::MIN) };
    // Model space only (paper space layouts repeat it).
    let mut model = dw.entities().filter(|e| !e.common.is_in_paper_space);
    draw(&dw, &mut model, Tf::ID, &layer_colors, 7, &mut svg, 0);
    if svg.lo.0 > svg.hi.0 {
        return Err(FileError::new("empty"));
    }
    let (w, h) = ((svg.hi.0 - svg.lo.0).max(1e-6), (svg.hi.1 - svg.lo.1).max(1e-6));
    let pad = w.max(h) * 0.03;
    let (vx, vy, vw, vh) = (svg.lo.0 - pad, -(svg.hi.1 + pad), w + 2.0 * pad, h + 2.0 * pad);
    let stroke = vw.max(vh) / 1500.0;
    // About 1000 px on the long side by default; PDF and PNG scale it.
    let k = 1000.0 / vw.max(vh);
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.2}\" height=\"{:.2}\" viewBox=\"{vx:.4} {vy:.4} {vw:.4} {vh:.4}\">\n<rect x=\"{vx:.4}\" y=\"{vy:.4}\" width=\"{vw:.4}\" height=\"{vh:.4}\" fill=\"#ffffff\"/>\n<g transform=\"scale(1 -1)\" stroke-width=\"{stroke:.5}\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n{}</g>\n</svg>\n",
        vw * k, vh * k, svg.out
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stl_round_trip() {
        let m = Mesh {
            pos: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            tri: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
        };
        let back = read_stl(&write_stl(&m)).unwrap();
        assert_eq!(back.pos.len(), 4);
        assert_eq!(back.tri.len(), 4);
        let obj = read_obj(&write_obj(&m));
        assert_eq!((obj.pos.len(), obj.tri.len()), (4, 4));
        let ply = read_ply(&write_ply(&m)).unwrap();
        assert_eq!((ply.pos.len(), ply.tri.len()), (4, 4));
    }

    #[test]
    fn bulge_half_circle() {
        // From (0,0) to (2,0) with bulge 1: a half circle of radius 1 through (1,-1).
        let pts = bulge_points(&[(0.0, 0.0, 1.0), (2.0, 0.0, 0.0)], false);
        assert!(pts.iter().any(|p| (p.0 - 1.0).abs() < 0.05 && (p.1 + 1.0).abs() < 0.05), "{pts:?}");
    }

    #[test]
    fn mtext() {
        assert_eq!(mtext_plain("{\\fArial|b0;\\H2.5;Line one}\\PLine \\U+0219two"), "Line one\nLine ștwo");
    }
}
