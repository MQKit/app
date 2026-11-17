//! Minimal HEIF (ISOBMFF) reader: the `meta` box with its items, locations,
//! references and properties. Used for the EXIF item (HEIC → JPG keeps dates,
//! camera and location) and to decode HEIC through FFmpeg (see `heic_ffmpeg`).

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn be(b: &[u8], o: usize, n: usize) -> Option<u64> {
    let s = b.get(o..o.checked_add(n)?)?;
    Some(s.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
}

/// Iterates boxes in `data[start..end]`: (type, content start, box end).
fn boxes(data: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let end = end.min(data.len());
    let mut out = Vec::new();
    let mut p = start;
    while p + 8 <= end {
        let Some(mut size) = be(data, p, 4) else { break };
        let ty: [u8; 4] = data[p + 4..p + 8].try_into().unwrap();
        let mut header = 8;
        if size == 1 {
            let Some(s) = be(data, p + 8, 8) else { break };
            size = s;
            header = 16;
        } else if size == 0 {
            size = (end - p) as u64;
        }
        if size < header as u64 {
            break;
        }
        let bend = p.saturating_add(usize::try_from(size).unwrap_or(usize::MAX)).min(end);
        out.push((ty, p + header, bend));
        p = bend;
    }
    out
}

/// Sequential big-endian reader over `b[p..end]`.
struct Rd<'a> {
    b: &'a [u8],
    p: usize,
    end: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8], p: usize, end: usize) -> Self {
        Rd { b, p, end: end.min(b.len()) }
    }
    fn u(&mut self, n: usize) -> Option<u64> {
        if n == 0 {
            return Some(0);
        }
        if self.p + n > self.end {
            return None;
        }
        let v = be(self.b, self.p, n)?;
        self.p += n;
        Some(v)
    }
    fn u8(&mut self) -> Option<u8> {
        self.u(1).map(|v| v as u8)
    }
    fn u16(&mut self) -> Option<u16> {
        self.u(2).map(|v| v as u16)
    }
    fn u32(&mut self) -> Option<u32> {
        self.u(4).map(|v| v as u32)
    }
    /// 16-bit for `version == 0`, else 32-bit (item ids, counts).
    fn id(&mut self, version: u8) -> Option<u32> {
        if version == 0 {
            self.u16().map(u32::from)
        } else {
            self.u32()
        }
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.p + n > self.end {
            return None;
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Some(s)
    }
}

/// An item property (`ipco` child).
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    /// HEVC decoder configuration: parameter-set NAL units (VPS/SPS/PPS/SEI, in order),
    /// NAL length size in bytes, bit depth (luma), chroma format (0 = 4:0:0 … 3 = 4:4:4).
    Hvcc { nals: Vec<Vec<u8>>, length_size: u8, bit_depth: u8, chroma: u8 },
    /// Image spatial extents (coded width, height).
    Ispe(u32, u32),
    /// Rotation, counter-clockwise, in units of 90°.
    Irot(u8),
    /// Mirroring: 0 = top and bottom exchanged, 1 = left and right exchanged.
    Imir(u8),
    /// Clean aperture: width, height (N/D) and the center offset (N/D, signed).
    Clap { w: (u32, u32), h: (u32, u32), x: (i32, u32), y: (i32, u32) },
    Nclx { primaries: u16, transfer: u16, matrix: u16, full_range: bool },
    Icc(Vec<u8>),
    /// Auxiliary type (`auxC`), e.g. alpha or depth.
    AuxC(String),
    Other([u8; 4]),
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub id: u32,
    pub ty: [u8; 4],
    pub hidden: bool,
}

#[derive(Clone, Debug, Default)]
struct Loc {
    method: u8,
    base: u64,
    extents: Vec<(u64, u64)>,
}

/// The parsed `meta` box.
#[derive(Debug, Default)]
pub struct Meta {
    pub primary: u32,
    /// In `iinf` order.
    pub items: Vec<Item>,
    locs: HashMap<u32, Loc>,
    /// (type, from, to…)
    refs: Vec<([u8; 4], u32, Vec<u32>)>,
    props: Vec<Prop>,
    /// item id → (essential, 1-based property index), in association order
    assoc: HashMap<u32, Vec<(bool, u16)>>,
    idat: Vec<u8>,
}

/// `grid` derived image descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub rows: u32,
    pub cols: u32,
    pub width: u32,
    pub height: u32,
}

impl Grid {
    pub fn parse(d: &[u8]) -> Option<Grid> {
        let mut r = Rd::new(d, 0, d.len());
        let _version = r.u8()?;
        let flags = r.u8()?;
        let rows = r.u8()? as u32 + 1;
        let cols = r.u8()? as u32 + 1;
        let (width, height) = if flags & 1 != 0 { (r.u32()?, r.u32()?) } else { (r.u16()? as u32, r.u16()? as u32) };
        Some(Grid { rows, cols, width, height })
    }
}

/// Crop rectangle (x, y, width, height) of a `clap` on a `w`×`h` image.
pub fn clap_rect(p: &Prop, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let Prop::Clap { w: cw, h: ch, x, y } = p else { return None };
    if cw.1 == 0 || ch.1 == 0 || x.1 == 0 || y.1 == 0 {
        return None;
    }
    let cw = ((cw.0 as f64 / cw.1 as f64).round() as u32).clamp(1, w.max(1));
    let ch = ((ch.0 as f64 / ch.1 as f64).round() as u32).clamp(1, h.max(1));
    // center = offset + (size - 1) / 2; left = center - (clean size - 1) / 2
    let left = (x.0 as f64 / x.1 as f64 + (w as f64 - cw as f64) / 2.0 + 1e-9).floor();
    let top = (y.0 as f64 / y.1 as f64 + (h as f64 - ch as f64) / 2.0 + 1e-9).floor();
    let left = left.clamp(0.0, (w - cw) as f64) as u32;
    let top = top.clamp(0.0, (h - ch) as f64) as u32;
    Some((left, top, cw, ch))
}

impl Meta {
    /// Parses the content of a `meta` box (starting with its version and flags).
    pub fn parse(m: &[u8]) -> Option<Meta> {
        let mut meta = Meta::default();
        for (ty, s, e) in boxes(m, 4, m.len()) {
            match &ty {
                b"pitm" => {
                    let mut r = Rd::new(m, s, e);
                    let v = r.u8()?;
                    r.u(3)?;
                    meta.primary = r.id(v)?;
                }
                b"iinf" => meta.parse_iinf(m, s, e)?,
                b"iloc" => meta.parse_iloc(m, s, e)?,
                b"iref" => meta.parse_iref(m, s, e)?,
                b"iprp" => meta.parse_iprp(m, s, e)?,
                b"idat" => meta.idat = m[s..e].to_vec(),
                _ => {}
            }
        }
        Some(meta)
    }

    fn parse_iinf(&mut self, m: &[u8], s: usize, e: usize) -> Option<()> {
        let v = *m.get(s)?;
        let first = s + 4 + if v == 0 { 2 } else { 4 };
        for (ty, cs, ce) in boxes(m, first, e) {
            if &ty != b"infe" {
                continue;
            }
            let mut r = Rd::new(m, cs, ce);
            let ver = r.u8()?;
            let flags = r.u(3)?;
            let id = match ver {
                2 => r.u16()? as u32,
                3 => r.u32()?,
                _ => continue, // versions 0/1 carry no item type
            };
            r.u16()?; // item_protection_index
            let ty: [u8; 4] = r.bytes(4)?.try_into().ok()?;
            self.items.push(Item { id, ty, hidden: flags & 1 != 0 });
        }
        Some(())
    }

    fn parse_iloc(&mut self, m: &[u8], s: usize, e: usize) -> Option<()> {
        let mut r = Rd::new(m, s, e);
        let ver = r.u8()?;
        r.u(3)?;
        let b0 = r.u8()?;
        let b1 = r.u8()?;
        let (off_sz, len_sz, base_sz) = ((b0 >> 4) as usize, (b0 & 15) as usize, (b1 >> 4) as usize);
        let idx_sz = if ver == 1 || ver == 2 { (b1 & 15) as usize } else { 0 };
        if [off_sz, len_sz, base_sz, idx_sz].iter().any(|&n| !matches!(n, 0 | 4 | 8)) {
            return None;
        }
        let count = if ver < 2 { r.u16()? as u32 } else { r.u32()? };
        for _ in 0..count {
            let id = if ver < 2 { r.u16()? as u32 } else { r.u32()? };
            let method = if ver == 1 || ver == 2 { (r.u16()? & 15) as u8 } else { 0 };
            r.u16()?; // data_reference_index
            let base = r.u(base_sz)?;
            let n = r.u16()?;
            let mut extents = Vec::with_capacity(n as usize);
            for _ in 0..n {
                r.u(idx_sz)?;
                let off = r.u(off_sz)?;
                let len = r.u(len_sz)?;
                extents.push((off, len));
            }
            self.locs.insert(id, Loc { method, base, extents });
        }
        Some(())
    }

    fn parse_iref(&mut self, m: &[u8], s: usize, e: usize) -> Option<()> {
        let v = *m.get(s)?;
        for (ty, cs, ce) in boxes(m, s + 4, e) {
            let mut r = Rd::new(m, cs, ce);
            let from = r.id(v)?;
            let n = r.u16()?;
            let mut to = Vec::with_capacity(n as usize);
            for _ in 0..n {
                to.push(r.id(v)?);
            }
            self.refs.push((ty, from, to));
        }
        Some(())
    }

    fn parse_iprp(&mut self, m: &[u8], s: usize, e: usize) -> Option<()> {
        for (ty, cs, ce) in boxes(m, s, e) {
            match &ty {
                b"ipco" => {
                    for (pty, ps, pe) in boxes(m, cs, ce) {
                        self.props.push(parse_prop(m, pty, ps, pe).unwrap_or(Prop::Other(pty)));
                    }
                }
                b"ipma" => {
                    let mut r = Rd::new(m, cs, ce);
                    let v = r.u8()?;
                    let flags = r.u(3)?;
                    let n = r.u32()?;
                    for _ in 0..n {
                        let id = r.id(v)?;
                        let k = r.u8()?;
                        let list = self.assoc.entry(id).or_default();
                        for _ in 0..k {
                            if flags & 1 != 0 {
                                let x = r.u16()?;
                                list.push((x & 0x8000 != 0, x & 0x7fff));
                            } else {
                                let x = r.u8()?;
                                list.push((x & 0x80 != 0, (x & 0x7f) as u16));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Some(())
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Properties of item `id`, in association order.
    pub fn props_of(&self, id: u32) -> Vec<&Prop> {
        self.assoc
            .get(&id)
            .map(|l| l.iter().filter(|a| a.1 > 0).filter_map(|a| self.props.get(a.1 as usize - 1)).collect())
            .unwrap_or_default()
    }

    /// Items referenced by `from` with a reference of type `ty`, in order.
    pub fn refs(&self, ty: &[u8; 4], from: u32) -> Vec<u32> {
        self.refs.iter().filter(|r| &r.0 == ty && r.1 == from).flat_map(|r| r.2.iter().copied()).collect()
    }

    fn has_ref_from(&self, ty: &[u8; 4], from: u32) -> bool {
        self.refs.iter().any(|r| &r.0 == ty && r.1 == from)
    }

    pub fn ispe(&self, id: u32) -> Option<(u32, u32)> {
        self.props_of(id).into_iter().find_map(|p| if let Prop::Ispe(w, h) = p { Some((*w, *h)) } else { None })
    }

    /// The image items a viewer shows: the primary first, then the other image items
    /// that are not thumbnails, auxiliary images (alpha, depth, gain maps), tiles or hidden.
    pub fn top_level_images(&self) -> Vec<u32> {
        let is_image = |t: &[u8; 4]| matches!(t, b"hvc1" | b"grid" | b"iden");
        let tiles: HashSet<u32> = self.refs.iter().filter(|r| &r.0 == b"dimg").flat_map(|r| r.2.iter().copied()).collect();
        let mut out = Vec::new();
        if self.item(self.primary).is_some_and(|i| is_image(&i.ty)) {
            out.push(self.primary);
        }
        for it in &self.items {
            if it.id != self.primary
                && is_image(&it.ty)
                && !it.hidden
                && !tiles.contains(&it.id)
                && !self.has_ref_from(b"thmb", it.id)
                && !self.has_ref_from(b"auxl", it.id)
            {
                out.push(it.id);
            }
        }
        out
    }

    /// The transformative properties of item `id` (clap, irot, imir) in the order HEIF
    /// prescribes, whatever their order in `ipma` (Windows' encoder lists imir first,
    /// its decoder still applies irot before imir).
    pub fn transforms(&self, id: u32) -> Vec<&Prop> {
        let rank = |p: &Prop| match p {
            Prop::Clap { .. } => 0,
            Prop::Irot(_) => 1,
            Prop::Imir(_) => 2,
            _ => 3,
        };
        let mut v: Vec<&Prop> = self.props_of(id).into_iter().filter(|p| rank(p) < 3).collect();
        v.sort_by_key(|p| rank(p));
        v
    }

    /// (width, height) of item `id` after its transforms (clap, irot), without decoding.
    pub fn display_size(&self, id: u32) -> Option<(u32, u32)> {
        let (mut w, mut h) = match self.ispe(id) {
            Some(s) => s,
            None if self.item(id)?.ty == *b"iden" => self.display_size(*self.refs(b"dimg", id).first()?)?,
            None => return None,
        };
        for p in self.transforms(id) {
            match p {
                Prop::Clap { .. } => {
                    if let Some((_, _, cw, ch)) = clap_rect(p, w, h) {
                        (w, h) = (cw, ch);
                    }
                }
                Prop::Irot(r) if r % 2 == 1 => std::mem::swap(&mut w, &mut h),
                _ => {}
            }
        }
        Some((w, h))
    }

    /// The bytes of item `id` (construction method 0: file offsets, 1: `idat`).
    pub fn item_data<R: Read + Seek>(&self, id: u32, r: &mut R) -> Option<Vec<u8>> {
        let loc = self.locs.get(&id)?;
        let mut out = Vec::new();
        match loc.method {
            0 => {
                let file_len = r.seek(SeekFrom::End(0)).ok()?;
                for &(off, len) in &loc.extents {
                    let start = loc.base.checked_add(off)?;
                    let len = if len == 0 { file_len.checked_sub(start)? } else { len };
                    if start.checked_add(len)? > file_len {
                        return None;
                    }
                    r.seek(SeekFrom::Start(start)).ok()?;
                    let at = out.len();
                    out.resize(at + len as usize, 0);
                    r.read_exact(&mut out[at..]).ok()?;
                }
            }
            1 => {
                for &(off, len) in &loc.extents {
                    let start = usize::try_from(loc.base.checked_add(off)?).ok()?;
                    let end = if len == 0 { self.idat.len() } else { start.checked_add(len as usize)? };
                    out.extend_from_slice(self.idat.get(start..end)?);
                }
            }
            _ => return None,
        }
        Some(out)
    }
}

fn parse_prop(m: &[u8], ty: [u8; 4], s: usize, e: usize) -> Option<Prop> {
    let mut r = Rd::new(m, s, e);
    Some(match &ty {
        b"hvcC" => {
            r.bytes(16)?;
            let chroma = r.u8()? & 3;
            let bit_depth = (r.u8()? & 7) + 8;
            r.bytes(3)?;
            let length_size = (r.u8()? & 3) + 1;
            let arrays = r.u8()?;
            let mut nals = Vec::new();
            for _ in 0..arrays {
                r.u8()?;
                let n = r.u16()?;
                for _ in 0..n {
                    let len = r.u16()? as usize;
                    nals.push(r.bytes(len)?.to_vec());
                }
            }
            Prop::Hvcc { nals, length_size, bit_depth, chroma }
        }
        b"ispe" => {
            r.u32()?;
            Prop::Ispe(r.u32()?, r.u32()?)
        }
        b"irot" => Prop::Irot(r.u8()? & 3),
        b"imir" => Prop::Imir(r.u8()? & 1),
        b"clap" => {
            let mut v = [0u32; 8];
            for x in &mut v {
                *x = r.u32()?;
            }
            Prop::Clap { w: (v[0], v[1]), h: (v[2], v[3]), x: (v[4] as i32, v[5]), y: (v[6] as i32, v[7]) }
        }
        b"colr" => match r.bytes(4)? {
            b"nclx" => Prop::Nclx { primaries: r.u16()?, transfer: r.u16()?, matrix: r.u16()?, full_range: r.u8()? & 0x80 != 0 },
            b"prof" | b"rICC" => Prop::Icc(m[r.p..e].to_vec()),
            _ => Prop::Other(ty),
        },
        b"auxC" => {
            r.u32()?;
            let rest = &m[r.p..e];
            let n = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
            Prop::AuxC(String::from_utf8_lossy(&rest[..n]).into_owned())
        }
        _ => Prop::Other(ty),
    })
}

/// Reads and parses the top-level `meta` box of a HEIF file, without reading `mdat`.
pub fn read_meta<R: Read + Seek>(r: &mut R) -> Option<Meta> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    let mut p = 0u64;
    while p + 8 <= len {
        r.seek(SeekFrom::Start(p)).ok()?;
        let mut h = [0u8; 16];
        let n = (len - p).min(16) as usize;
        r.read_exact(&mut h[..n]).ok()?;
        let mut size = be(&h, 0, 4)?;
        let mut header = 8;
        if size == 1 {
            size = be(&h[..n], 8, 8)?;
            header = 16;
        } else if size == 0 {
            size = len - p;
        }
        if size < header || p + size > len {
            return None;
        }
        if &h[4..8] == b"meta" {
            let content = size - header;
            if content > 64 << 20 {
                return None;
            }
            r.seek(SeekFrom::Start(p + header)).ok()?;
            let mut buf = vec![0u8; content as usize];
            r.read_exact(&mut buf).ok()?;
            return Meta::parse(&buf);
        }
        p += size;
    }
    None
}

/// Returns the EXIF TIFF bytes (starting at `II*\0` / `MM\0*`), if present.
pub fn exif(data: &[u8]) -> Option<Vec<u8>> {
    let meta = read_meta(&mut Cursor::new(data))?;
    let id = meta.items.iter().find(|i| &i.ty == b"Exif")?.id;
    let buf = meta.item_data(id, &mut Cursor::new(data))?;
    let skip = be(&buf, 0, 4)? as usize;
    let tiff = buf.get(4usize.checked_add(skip)?..)?;
    if tiff.starts_with(b"II*\0") || tiff.starts_with(b"MM\0*") {
        return Some(tiff.to_vec());
    }
    // Some writers include "Exif\0\0" before the TIFF header.
    let pos = tiff.windows(4).position(|w| w == b"II*\0" || w == b"MM\0*")?;
    Some(tiff[pos..].to_vec())
}

/// Test helper: writes HEIF files (ftyp, meta, mdat) from items, properties and references.
#[cfg(test)]
pub mod writer {
    pub fn bx(ty: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut v = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(content);
        v
    }

    pub fn full(ty: &[u8; 4], version: u8, flags: u32, content: &[u8]) -> Vec<u8> {
        let mut c = vec![version];
        c.extend_from_slice(&flags.to_be_bytes()[1..]);
        c.extend_from_slice(content);
        bx(ty, &c)
    }

    pub struct WItem {
        pub id: u16,
        pub ty: [u8; 4],
        pub hidden: bool,
        pub data: Vec<u8>,
        /// Stored in `idat` (construction method 1) instead of `mdat`.
        pub in_idat: bool,
        /// 1-based indexes into `props`, essential flag.
        pub props: Vec<(u8, bool)>,
    }

    pub struct Heif {
        pub primary: u16,
        pub items: Vec<WItem>,
        /// Full property boxes (e.g. from [`ispe`], [`irot`]).
        pub props: Vec<Vec<u8>>,
        /// (type, from, to…)
        pub refs: Vec<([u8; 4], u16, Vec<u16>)>,
    }

    pub fn ispe(w: u32, h: u32) -> Vec<u8> {
        let mut c = w.to_be_bytes().to_vec();
        c.extend_from_slice(&h.to_be_bytes());
        full(b"ispe", 0, 0, &c)
    }
    pub fn irot(r: u8) -> Vec<u8> {
        bx(b"irot", &[r & 3])
    }
    pub fn imir(axis: u8) -> Vec<u8> {
        bx(b"imir", &[axis & 1])
    }
    pub fn clap(w: (u32, u32), h: (u32, u32), x: (i32, u32), y: (i32, u32)) -> Vec<u8> {
        let mut c = Vec::new();
        for v in [w.0, w.1, h.0, h.1, x.0 as u32, x.1, y.0 as u32, y.1] {
            c.extend_from_slice(&v.to_be_bytes());
        }
        bx(b"clap", &c)
    }
    pub fn nclx(primaries: u16, transfer: u16, matrix: u16, full_range: bool) -> Vec<u8> {
        let mut c = b"nclx".to_vec();
        for v in [primaries, transfer, matrix] {
            c.extend_from_slice(&v.to_be_bytes());
        }
        c.push(if full_range { 0x80 } else { 0 });
        bx(b"colr", &c)
    }
    pub fn aux_c(urn: &str) -> Vec<u8> {
        let mut c = urn.as_bytes().to_vec();
        c.push(0);
        full(b"auxC", 0, 0, &c)
    }
    /// hvcC from VPS/SPS/PPS NAL units (without start codes); profile fields from the SPS.
    pub fn hvcc(vps: &[u8], sps: &[u8], pps: &[u8], chroma: u8, bit_depth: u8) -> Vec<u8> {
        // SPS RBSP without emulation prevention bytes: [2-byte header][1 byte][profile_tier_level…]
        let mut rbsp = Vec::new();
        let mut zeros = 0;
        for &b in sps {
            if zeros >= 2 && b == 3 {
                zeros = 0;
                continue;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            rbsp.push(b);
        }
        let mut c = vec![1u8];
        c.extend_from_slice(&rbsp[3..15]); // profile byte, 4 compat, 6 constraint, level
        c.extend_from_slice(&[0xF0, 0x00, 0xFC, 0xFC | chroma, 0xF8 | (bit_depth - 8), 0xF8 | (bit_depth - 8), 0, 0, 0x0F, 3]);
        for (t, n) in [(32u8, vps), (33, sps), (34, pps)] {
            c.push(0x80 | t);
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&(n.len() as u16).to_be_bytes());
            c.extend_from_slice(n);
        }
        bx(b"hvcC", &c)
    }
    pub fn grid(rows: u8, cols: u8, w: u32, h: u32) -> Vec<u8> {
        if w > 0xFFFF || h > 0xFFFF {
            let mut v = vec![0, 1, rows - 1, cols - 1];
            v.extend_from_slice(&w.to_be_bytes());
            v.extend_from_slice(&h.to_be_bytes());
            v
        } else {
            let mut v = vec![0, 0, rows - 1, cols - 1];
            v.extend_from_slice(&(w as u16).to_be_bytes());
            v.extend_from_slice(&(h as u16).to_be_bytes());
            v
        }
    }

    impl Heif {
        fn meta(&self, mdat_data: u64) -> Vec<u8> {
            let mut c = full(b"hdlr", 0, 0, &[0, 0, 0, 0, b'p', b'i', b'c', b't', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            c.extend(full(b"pitm", 0, 0, &self.primary.to_be_bytes()));
            let mut iinf = (self.items.len() as u16).to_be_bytes().to_vec();
            for it in &self.items {
                let mut e = it.id.to_be_bytes().to_vec();
                e.extend_from_slice(&[0, 0]);
                e.extend_from_slice(&it.ty);
                e.push(0);
                iinf.extend(full(b"infe", 2, it.hidden as u32, &e));
            }
            c.extend(full(b"iinf", 0, 0, &iinf));
            // iloc v1: offset 4, length 4, base 0, index 0
            let mut iloc = vec![0x44, 0x00];
            iloc.extend_from_slice(&(self.items.len() as u16).to_be_bytes());
            let (mut mdat_off, mut idat_off) = (mdat_data, 0u64);
            for it in &self.items {
                iloc.extend_from_slice(&it.id.to_be_bytes());
                iloc.extend_from_slice(&(it.in_idat as u16).to_be_bytes());
                iloc.extend_from_slice(&[0, 0, 0, 1]);
                let off = if it.in_idat { &mut idat_off } else { &mut mdat_off };
                iloc.extend_from_slice(&(*off as u32).to_be_bytes());
                iloc.extend_from_slice(&(it.data.len() as u32).to_be_bytes());
                *off += it.data.len() as u64;
            }
            c.extend(full(b"iloc", 1, 0, &iloc));
            let mut ipco = Vec::new();
            for p in &self.props {
                ipco.extend_from_slice(p);
            }
            let mut ipma = (self.items.len() as u32).to_be_bytes().to_vec();
            for it in &self.items {
                ipma.extend_from_slice(&it.id.to_be_bytes());
                ipma.push(it.props.len() as u8);
                for &(i, ess) in &it.props {
                    ipma.push(i | if ess { 0x80 } else { 0 });
                }
            }
            let mut iprp = bx(b"ipco", &ipco);
            iprp.extend(full(b"ipma", 0, 0, &ipma));
            c.extend(bx(b"iprp", &iprp));
            let mut iref = Vec::new();
            for (ty, from, to) in &self.refs {
                let mut r = from.to_be_bytes().to_vec();
                r.extend_from_slice(&(to.len() as u16).to_be_bytes());
                for t in to {
                    r.extend_from_slice(&t.to_be_bytes());
                }
                iref.extend(bx(ty, &r));
            }
            c.extend(full(b"iref", 0, 0, &iref));
            let idat: Vec<u8> = self.items.iter().filter(|i| i.in_idat).flat_map(|i| i.data.clone()).collect();
            if !idat.is_empty() {
                c.extend(bx(b"idat", &idat));
            }
            full(b"meta", 0, 0, &c)
        }

        pub fn to_bytes(&self) -> Vec<u8> {
            let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
            let first = self.meta(0).len() as u64;
            let meta = self.meta(ftyp.len() as u64 + first + 8);
            let mut out = ftyp;
            out.extend(meta);
            let mdat: Vec<u8> = self.items.iter().filter(|i| !i.in_idat).flat_map(|i| i.data.clone()).collect();
            out.extend(bx(b"mdat", &mdat));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::writer::*;
    use super::*;

    fn item(id: u16, ty: &[u8; 4], data: Vec<u8>, props: Vec<(u8, bool)>) -> WItem {
        WItem { id, ty: *ty, hidden: false, data, in_idat: false, props }
    }

    fn sample() -> Vec<u8> {
        let hv = bx(b"hvcC", &{
            let mut c = vec![1u8];
            c.extend_from_slice(&[0; 15]);
            c.extend_from_slice(&[0xFD, 0xF8, 0xF8, 0, 0, 0x0F, 2]);
            c.extend_from_slice(&[0xA0, 0, 1, 0, 3, 0x40, 0x01, 0x0C]); // VPS
            c.extend_from_slice(&[0xA1, 0, 1, 0, 2, 0x42, 0x01]); // SPS
            c
        });
        let mut tiles: Vec<WItem> = (1..=4).map(|i| item(i, b"hvc1", vec![i as u8; 10], vec![(1, true), (2, false)])).collect();
        tiles[3].hidden = true;
        let mut g = item(10, b"grid", grid(2, 2, 1000, 900), vec![(3, false), (4, true), (5, true), (6, false)]);
        g.in_idat = true;
        let mut exif = b"\0\0\0\x06Exif\0\0MM\0*\0\0\0\x08".to_vec();
        exif.extend_from_slice(&[0; 4]);
        Heif {
            primary: 10,
            items: vec![
                tiles.remove(0),
                tiles.remove(0),
                tiles.remove(0),
                tiles.remove(0),
                g,
                item(11, b"hvc1", vec![7; 5], vec![(1, true), (7, false)]), // thumbnail
                item(12, b"hvc1", vec![8; 5], vec![(1, true), (8, true)]),  // alpha
                item(13, b"hvc1", vec![9; 5], vec![(1, true), (2, false)]), // burst frame
                item(14, b"Exif", exif, vec![]),
            ],
            props: vec![
                hv,
                ispe(512, 512),
                ispe(1000, 900),
                clap((801, 1), (600, 1), (-50, 1), (10, 2)),
                irot(1),
                imir(1),
                ispe(100, 90),
                aux_c("urn:mpeg:hevc:2015:auxid:1"),
                nclx(1, 13, 6, true),
            ],
            refs: vec![
                (*b"dimg", 10, vec![3, 1, 4, 2]),
                (*b"thmb", 11, vec![10]),
                (*b"auxl", 12, vec![10]),
                (*b"cdsc", 14, vec![10]),
            ],
        }
        .to_bytes()
    }

    #[test]
    fn parses_items_refs_and_props() {
        let d = sample();
        let m = read_meta(&mut Cursor::new(&d)).unwrap();
        assert_eq!(m.primary, 10);
        assert_eq!(m.items.len(), 9);
        assert!(m.item(4).unwrap().hidden);
        assert_eq!(&m.item(10).unwrap().ty, b"grid");
        assert_eq!(m.refs(b"dimg", 10), vec![3, 1, 4, 2]);
        assert_eq!(m.top_level_images(), vec![10, 13]);
        let Prop::Hvcc { nals, length_size, bit_depth, chroma } = m.props_of(1)[0] else { panic!() };
        assert_eq!((nals.len(), *length_size, *bit_depth, *chroma), (2, 4, 8, 1));
        assert_eq!(nals[0], vec![0x40, 0x01, 0x0C]);
        assert_eq!(m.ispe(1), Some((512, 512)));
        let p = m.props_of(10);
        assert_eq!(p.len(), 4);
        assert_eq!(p[2], &Prop::Irot(1));
        assert_eq!(p[3], &Prop::Imir(1));
        // clap 801×600 centered -50, +5 on 1000×900: left = -50 + 99.5 → 49, top = 5 + 150 = 155
        assert_eq!(clap_rect(p[1], 1000, 900), Some((49, 155, 801, 600)));
        assert_eq!(m.display_size(10), Some((600, 801)));
        assert_eq!(m.display_size(13), Some((512, 512)));
        assert!(m.props_of(13).iter().all(|p| !matches!(p, Prop::Nclx { .. })));
    }

    #[test]
    fn reads_item_data_from_mdat_and_idat() {
        let d = sample();
        let m = read_meta(&mut Cursor::new(&d)).unwrap();
        assert_eq!(m.item_data(3, &mut Cursor::new(&d)).unwrap(), vec![3u8; 10]);
        let g = Grid::parse(&m.item_data(10, &mut Cursor::new(&d)).unwrap()).unwrap();
        assert_eq!(g, Grid { rows: 2, cols: 2, width: 1000, height: 900 });
        assert_eq!(Grid::parse(&grid(8, 6, 70000, 3000)), Some(Grid { rows: 8, cols: 6, width: 70000, height: 3000 }));
        assert_eq!(exif(&d).unwrap(), b"MM\0*\0\0\0\x08\0\0\0\0".to_vec());
    }

    #[test]
    fn rejects_truncated_input() {
        let d = sample();
        // cut inside meta: no panic, no result
        for cut in [10, 40, 100, 300] {
            let _ = read_meta(&mut Cursor::new(&d[..cut]));
        }
        assert!(read_meta(&mut Cursor::new(b"\0\0\0\x08ftyp")).is_none());
        let m = read_meta(&mut Cursor::new(&d)).unwrap();
        // data outside the file
        assert!(m.item_data(14, &mut Cursor::new(&d[..d.len() - 30])).is_none());
    }

    #[test]
    fn reads_windows_encoded_grid() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/in/iphone.heic");
        let Ok(d) = std::fs::read(&p) else { return };
        let m = read_meta(&mut Cursor::new(&d)).unwrap();
        let g = Grid::parse(&m.item_data(m.primary, &mut Cursor::new(&d)).unwrap()).unwrap();
        assert_eq!((g.rows * g.cols) as usize, m.refs(b"dimg", m.primary).len());
        assert_eq!(m.top_level_images(), vec![m.primary]);
        assert!(exif(&d).is_some());
    }
}
