//! Before/after previews (contract section 10, `target.kind = crop`): a region at
//! 100 %, the original next to the result encoded with the current options.

use contract::{codes, FileError, Request};
use image::DynamicImage;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The region to show: `target.rect` (relative 0–1) or the centre, at most `maxPx` square.
pub fn crop(img: &DynamicImage, req: &Request) -> DynamicImage {
    let target = req.target.clone().unwrap_or(Value::Null);
    let max = target.get("maxPx").and_then(Value::as_u64).unwrap_or(480).clamp(64, 1600) as u32;
    let (w, h) = (img.width(), img.height());
    let (cw, ch) = (w.min(max), h.min(max));
    let (cx, cy) = match target.get("rect").and_then(Value::as_array) {
        Some(r) if r.len() == 4 => {
            let f = |i: usize| r[i].as_f64().unwrap_or(0.5);
            (((f(0) + f(2)) / 2.0 * w as f64) as u32, ((f(1) + f(3)) / 2.0 * h as f64) as u32)
        }
        _ => (w / 2, h / 2),
    };
    let x = cx.saturating_sub(cw / 2).min(w - cw);
    let y = cy.saturating_sub(ch / 2).min(h - ch);
    img.crop_imm(x, y, cw, ch)
}

/// Writes `before.png` and the decoded `after` into `previewDir`; returns the `preview` message.
pub fn write(req: &Request, before: &DynamicImage, after_encoded: &[u8], after_ext: &str) -> Result<Value, FileError> {
    let dir: PathBuf = req.preview_dir.clone().ok_or_else(|| FileError::new(codes::INVALID_OPTIONS))?;
    std::fs::create_dir_all(&dir).map_err(crate::plugins::io_err)?;
    let id = req.id.clone().unwrap_or_else(|| "p".into());
    let after = image::load_from_memory(after_encoded).map_err(crate::common::img::img_err)?;
    let b = dir.join(format!("{id}-before.png"));
    let a = dir.join(format!("{id}-after.png"));
    before.save(&b).map_err(crate::common::img::img_err)?;
    after.save(&a).map_err(crate::common::img::img_err)?;
    Ok(json!({
        "type": "preview", "id": id, "before": b, "path": a,
        "width": before.width(), "height": before.height(),
        "ext": after_ext, "cropBytes": after_encoded.len()
    }))
}
