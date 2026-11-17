//! Tests of `heic_ffmpeg`: HEIC files built here (tiles encoded with libx265, container
//! from `heif::writer`) and Windows-encoded grids, compared by PSNR.

use super::*;
use crate::common::heif::writer::*;
use contract::Request;

fn ffmpeg() -> Option<PathBuf> {
    crate::common::ffmpeg::exe(&Request::default(), "ffmpeg").ok()
}

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/in").join(name)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("convertor-heic-tests");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn source(name: &str, x: u32, y: u32, w: u32, h: u32) -> Option<RgbImage> {
    let img = image::open(testdata(name)).ok()?.to_rgb8();
    Some(image::imageops::crop_imm(&img, x, y, w, h).to_image())
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mse = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-10)).log10()
}

fn rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

/// NAL units of an Annex-B stream (without start codes).
fn split_annex_b(d: &[u8]) -> Vec<&[u8]> {
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 <= d.len() {
        if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    (0..starts.len())
        .map(|k| {
            let s = starts[k];
            let mut e = if k + 1 < starts.len() { starts[k + 1] - 3 } else { d.len() };
            while e > s && d[e - 1] == 0 {
                e -= 1;
            }
            &d[s..e]
        })
        .collect()
}

struct Encoded {
    hvcc: Vec<u8>,
    /// Per tile: 4-byte length-prefixed NAL units.
    tiles: Vec<Vec<u8>>,
}

/// Encodes `img` cut into `cols`×`rows` tiles with libx265 (all intra, one picture per
/// tile, row-major). YUV is full range BT.709; the SPS VUI says so for `sps_range` "pc",
/// and says limited range, unspecified matrix for "tv".
fn encode(ff: &Path, img: &RgbImage, cols: u32, rows: u32, sps_range: &str) -> Encoded {
    let png = scratch(&format!("src-{}x{}-{}x{}-{}.png", img.width(), img.height(), cols, rows, sps_range));
    img.save(&png).unwrap();
    let out = png.with_extension("hevc");
    // "tv": the SPS is wrong (limited, no matrix), a colr nclx must correct it
    let cs = if sps_range == "tv" { "unknown" } else { "bt709" };
    let vf = format!("untile={cols}x{rows},scale=out_range=pc:out_color_matrix=bt709,format=yuv420p,setparams=range={sps_range}:colorspace={cs}");
    let st = crate::common::ffmpeg::command(ff)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&png)
        .args(["-vf", &vf, "-c:v", "libx265", "-crf", "10", "-preset", "fast"])
        .args(["-x265-params", "keyint=1:bframes=0:open-gop=0:log-level=error", "-f", "hevc"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(st.success());
    let d = std::fs::read(&out).unwrap();
    let (mut vps, mut sps, mut pps) = (vec![], vec![], vec![]);
    let mut tiles: Vec<Vec<u8>> = vec![];
    for n in split_annex_b(&d) {
        match (n[0] >> 1) & 0x3f {
            32 => vps = n.to_vec(),
            33 => sps = n.to_vec(),
            34 => pps = n.to_vec(),
            0..=31 => {
                if n[2] & 0x80 != 0 {
                    tiles.push(vec![]);
                }
                let t = tiles.last_mut().unwrap();
                t.extend_from_slice(&(n.len() as u32).to_be_bytes());
                t.extend_from_slice(n);
            }
            _ => {} // SEI, AUD
        }
    }
    assert_eq!(tiles.len(), (cols * rows) as usize);
    Encoded { hvcc: hvcc(&vps, &sps, &pps, 1, 8), tiles }
}

// Reference transforms, written independently of `transform` (pixel by pixel).
fn crop(img: &RgbImage, x: u32, y: u32, w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |i, j| *img.get_pixel(x + i, y + j))
}
fn rot_ccw(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    // the top-right corner becomes the top-left one
    RgbImage::from_fn(h, w, |i, j| *img.get_pixel(w - 1 - j, i))
}
fn mirror_lr(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    RgbImage::from_fn(w, h, |i, j| *img.get_pixel(w - 1 - i, j))
}

/// iPhone-like grid: 3×2 tiles of 512×512 (ids not in raster order), output 1500×1000,
/// clap, irot = 1, and a colr nclx saying full range BT.709 while the SPS says limited.
#[test]
fn decodes_grid_with_clap_irot_and_nclx_range() {
    let (Some(ff), Some(canvas)) = (ffmpeg(), source("tema.jpg", 1000, 600, 1536, 1024)) else { return };
    let enc = encode(&ff, &canvas, 3, 2, "tv");
    let mut items: Vec<WItem> = enc
        .tiles
        .into_iter()
        .enumerate()
        .map(|(i, data)| WItem { id: 100 - i as u16, ty: *b"hvc1", hidden: true, data, in_idat: false, props: vec![(1, true), (2, false)] })
        .collect();
    let tile_ids: Vec<u16> = items.iter().map(|i| i.id).collect();
    items.push(WItem {
        id: 7,
        ty: *b"grid",
        hidden: false,
        data: grid(2, 3, 1500, 1000),
        in_idat: true,
        props: vec![(3, false), (6, false), (4, true), (5, true)],
    });
    let mut file = Heif {
        primary: 7,
        items,
        props: vec![enc.hvcc, ispe(512, 512), ispe(1500, 1000), clap((1400, 1), (900, 1), (40, 2), (-10, 1)), irot(1), nclx(1, 13, 1, true)],
        refs: vec![(*b"dimg", 7, tile_ids)],
    };
    let path = scratch("grid-irot.heic");
    std::fs::write(&path, file.to_bytes()).unwrap();

    assert_eq!(image_count(&path).unwrap(), 1);
    assert_eq!(dimensions(&path).unwrap(), (900, 1400));
    let mut seen = vec![];
    let t0 = std::time::Instant::now();
    let (w, h, px) = decode_with(&ff, &path, 0, &mut |x| {
        seen.push(x);
        true
    })
    .unwrap();
    eprintln!("grid 3x2 decoded in {:?}", t0.elapsed());
    assert_eq!((w, h), (900, 1400));
    assert!(seen.windows(2).all(|p| p[1] >= p[0]) && seen.last() == Some(&1.0) && seen.len() > 2, "{seen:?}");
    // clap 1400×900 centered +20, -10 on 1500×1000 → (70, 40); then 90° counter-clockwise
    let expected = rot_ccw(&crop(&canvas, 70, 40, 1400, 900));
    let db = psnr(&rgb(&px), expected.as_raw());
    eprintln!("grid PSNR {db:.2} dB");
    assert!(db > 30.0, "PSNR {db}");
    // without the nclx, the full-range samples are read as limited range: clearly worse
    file.items.last_mut().unwrap().props.retain(|p| p.0 != 6);
    std::fs::write(&path, file.to_bytes()).unwrap();
    let (_, _, px2) = decode(&ff, &path, 0).unwrap();
    let db2 = psnr(&rgb(&px2), expected.as_raw());
    eprintln!("grid PSNR without nclx {db2:.2} dB");
    assert!(db2 < db - 5.0);
    if std::env::var("CONVERTOR_KEEP_TEST_HEIC").is_ok() {
        image::RgbaImage::from_raw(w, h, px).unwrap().save(scratch("grid-irot.png")).unwrap();
    }
}

/// One HEVC image with imir listed before irot (like Windows writes it), a clap, SPS
/// full range; a second top-level image (burst), a thumbnail and an alpha plane.
#[test]
fn decodes_single_image_burst_thumbnail_and_alpha() {
    let (Some(ff), Some(a), Some(b)) = (ffmpeg(), source("peisaj ăîș.jpg", 700, 500, 1024, 768), source("tema.jpg", 2000, 1000, 512, 512)) else {
        return;
    };
    let ea = encode(&ff, &a, 1, 1, "pc");
    let eb = encode(&ff, &b, 1, 1, "pc");
    let thumb = eb.tiles[0].clone();
    let item = |id: u16, data: Vec<u8>, props: Vec<(u8, bool)>| WItem { id, ty: *b"hvc1", hidden: false, data, in_idat: false, props };
    let file = Heif {
        primary: 1,
        items: vec![
            item(1, ea.tiles[0].clone(), vec![(1, true), (3, false), (6, true), (5, true), (7, true)]),
            item(2, thumb.clone(), vec![(2, true), (4, false)]),
            item(3, thumb, vec![(2, true), (4, false), (8, false)]),
            item(4, eb.tiles[0].clone(), vec![(2, true), (4, false)]),
            WItem { id: 5, ty: *b"Exif", hidden: false, data: vec![0, 0, 0, 0, b'M', b'M', 0, 42, 0, 0, 0, 8], in_idat: false, props: vec![] },
        ],
        props: vec![
            ea.hvcc,
            eb.hvcc,
            ispe(1024, 768),
            ispe(512, 512),
            irot(2),
            imir(1),
            clap((1000, 1), (700, 1), (0, 1), (6, 1)),
            aux_c("urn:mpeg:hevc:2015:auxid:1"),
        ],
        refs: vec![(*b"thmb", 2, vec![1]), (*b"auxl", 3, vec![1]), (*b"cdsc", 5, vec![1])],
    };
    let path = scratch("single-imir.heic");
    std::fs::write(&path, file.to_bytes()).unwrap();

    assert_eq!(image_count(&path).unwrap(), 2);
    assert_eq!(dimensions(&path).unwrap(), (1000, 700));
    let (w, h, px) = decode(&ff, &path, 0).unwrap();
    assert_eq!((w, h), (1000, 700));
    assert!(px.chunks_exact(4).all(|p| p[3] == 255));
    // clap (12, 40, 1000×700), irot 180°, then mirror left-right
    let expected = mirror_lr(&rot_ccw(&rot_ccw(&crop(&a, 12, 40, 1000, 700))));
    let db = psnr(&rgb(&px), expected.as_raw());
    eprintln!("single PSNR {db:.2} dB");
    assert!(db > 30.0, "PSNR {db}");
    let (w, h, px) = decode(&ff, &path, 1).unwrap();
    assert_eq!((w, h), (512, 512));
    let db = psnr(&rgb(&px), b.as_raw());
    eprintln!("burst PSNR {db:.2} dB");
    assert!(db > 30.0, "PSNR {db}");
    assert_eq!(decode(&ff, &path, 2).unwrap_err().code, codes::RANGE_OUT_OF_BOUNDS);
}

#[test]
fn errors() {
    let Some(ff) = ffmpeg() else { return };
    let p = scratch("not-heic.heic");
    std::fs::write(&p, b"\0\0\0\x10ftypheic\0\0\0\0junkjunkjunk").unwrap();
    assert_eq!(decode(&ff, &p, 0).unwrap_err().code, codes::CORRUPT_FILE);
    assert_eq!(image_count(&p).unwrap_err().code, codes::CORRUPT_FILE);
    // a 20000×20000 grid is refused before decoding
    let big = Heif {
        primary: 1,
        items: vec![WItem { id: 1, ty: *b"grid", hidden: false, data: grid(1, 1, 20000, 20000), in_idat: true, props: vec![(1, false)] }],
        props: vec![ispe(20000, 20000)],
        refs: vec![],
    };
    std::fs::write(&p, big.to_bytes()).unwrap();
    assert_eq!(decode(&ff, &p, 0).unwrap_err().code, codes::FILE_TOO_LARGE);
    if testdata("iphone.heic").is_file() {
        let e = decode(Path::new("C:\\nowhere\\ffmpeg.exe"), &testdata("iphone.heic"), 0).unwrap_err();
        assert_eq!(e.code, codes::ENGINE_MISSING);
        assert_eq!(e.details.unwrap()["component"], "ffmpeg");
    }
}

/// Windows-encoded grids (8×5 tiles, irot/imir for every EXIF orientation):
/// same size and pixels as Windows' own decoder.
#[test]
fn matches_windows_decoder() {
    let (Some(ff), true) = (ffmpeg(), testdata("tema.jpg").is_file()) else { return };
    if crate::common::winrt::decode_image(&testdata("iphone.heic"), 0).is_err() {
        return; // no Windows HEIF codec here
    }
    for o in 1..=8u16 {
        let p = scratch(&format!("windows-o{o}.heic"));
        crate::common::winrt::dev_encode_heic(&testdata("tema.jpg"), &p, o).unwrap();
        let (ww, wh, wpx) = crate::common::winrt::decode_image(&p, 0).unwrap();
        assert_eq!(dimensions(&p).unwrap(), (ww, wh), "orientation {o}");
        assert_eq!(image_count(&p).unwrap(), 1);
        let (w, h, px) = decode(&ff, &p, 0).unwrap();
        assert_eq!((w, h), (ww, wh), "orientation {o}");
        let db = psnr(&rgb(&px), &rgb(&wpx));
        eprintln!("orientation {o}: {w}x{h}, PSNR vs Windows {db:.2} dB");
        assert!(db > 38.0, "orientation {o}: PSNR {db}");
    }
}

/// The probe of `img::windows_heic`: decodable by our decoder (and by Windows when its
/// codec is installed). `CONVERTOR_WRITE_PROBE=1` regenerates the asset.
#[test]
fn heic_probe() {
    let Some(ff) = ffmpeg() else { return };
    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/heic-probe.heic");
    if std::env::var("CONVERTOR_WRITE_PROBE").is_ok() {
        let img = RgbImage::from_fn(64, 64, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 128]));
        let e = encode(&ff, &img, 1, 1, "pc");
        let file = Heif {
            primary: 1,
            items: vec![WItem { id: 1, ty: *b"hvc1", hidden: false, data: e.tiles[0].clone(), in_idat: false, props: vec![(1, true), (2, false)] }],
            props: vec![e.hvcc, ispe(64, 64)],
            refs: vec![],
        };
        std::fs::write(&asset, file.to_bytes()).unwrap();
    }
    let (w, h, _) = decode(&ff, &asset, 0).unwrap();
    assert_eq!((w, h), (64, 64));
    if crate::common::winrt::decode_image(&testdata("iphone.heic"), 0).is_ok() {
        assert_eq!(crate::common::winrt::decode_image(&asset, 0).map(|r| (r.0, r.1)).unwrap(), (64, 64));
        assert!(crate::common::img::windows_heic() || std::env::var("CONVERTOR_FORCE_HEIC_FFMPEG").is_ok());
    }
}
