//! The common page-range parser (qpdf-like syntax): `1-3,7`, `z` = last page,
//! `r1` = counted from the end, `:odd` / `:even` suffixes, `all` or empty = every page.

/// Parses `spec` into 1-based page numbers, in the written order (repetitions kept).
/// Returns `Err` with the offending part when a page is outside `1..=count` or the text is invalid.
pub fn parse_ordered(spec: &str, count: u32) -> Result<Vec<u32>, String> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
        return Ok((1..=count).collect());
    }
    // Written as people write it: "1 3 5", "2 – 4", "1;3", "5-" (to the end), "-3" (the first three).
    let mut norm = String::with_capacity(spec.len());
    for c in spec.chars() {
        let c = match c {
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            ';' => ',',
            c if c.is_whitespace() => ' ',
            c => c,
        };
        if (c == '-' || c == ':') && norm.ends_with(' ') {
            norm.truncate(norm.trim_end().len());
        }
        if c == ' ' && (norm.ends_with('-') || norm.ends_with(':') || norm.ends_with(',')) {
            continue;
        }
        norm.push(c);
    }
    let mut out = Vec::new();
    for raw in norm.split([',', ' ']) {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let (range, parity) = match part.split_once(':') {
            Some((r, p)) => (r.trim(), Some(p.trim().to_ascii_lowercase())),
            None => (part, None),
        };
        let (a, b) = match range.split_once('-') {
            Some((a, b)) if a.trim().is_empty() && b.trim().is_empty() => return Err(part.to_string()),
            Some((a, b)) if a.trim().is_empty() => (1, page(b, count)?),
            Some((a, b)) if b.trim().is_empty() => (page(a, count)?, count),
            Some((a, b)) => (page(a, count)?, page(b, count)?),
            None if range.is_empty() => (1, count),
            None => {
                let p = page(range, count)?;
                (p, p)
            }
        };
        let seq: Vec<u32> = if a <= b { (a..=b).collect() } else { (b..=a).rev().collect() };
        let mut idx = 0u32;
        for p in seq {
            idx += 1;
            let keep = match parity.as_deref() {
                Some("odd") => idx % 2 == 1,
                Some("even") => idx.is_multiple_of(2),
                Some(other) => return Err(format!(":{other}")),
                None => true,
            };
            if keep {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Like [`parse_ordered`], but sorted and without duplicates.
pub fn parse_set(spec: &str, count: u32) -> Result<Vec<u32>, String> {
    let mut v = parse_ordered(spec, count)?;
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

fn page(s: &str, count: u32) -> Result<u32, String> {
    let s = s.trim().to_ascii_lowercase();
    let n = if s == "z" {
        count
    } else if let Some(r) = s.strip_prefix('r') {
        let k: u32 = r.parse().map_err(|_| s.clone())?;
        if k == 0 || k > count {
            return Err(s);
        }
        count + 1 - k
    } else {
        s.parse().map_err(|_| s.clone())?
    };
    if n == 0 || n > count {
        return Err(s);
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        assert_eq!(parse_ordered("1-3,7", 10).unwrap(), vec![1, 2, 3, 7]);
        assert_eq!(parse_ordered("z", 5).unwrap(), vec![5]);
        assert_eq!(parse_ordered("r1-r3", 5).unwrap(), vec![5, 4, 3]);
        assert_eq!(parse_ordered(":odd", 5).unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_ordered("1-6:even", 9).unwrap(), vec![2, 4, 6]);
        assert_eq!(parse_set("3,1,3", 5).unwrap(), vec![1, 3]);
        assert_eq!(parse_ordered("", 3).unwrap(), vec![1, 2, 3]);
        assert!(parse_ordered("0", 3).is_err());
        assert!(parse_ordered("4", 3).is_err());
        assert!(parse_ordered("abc", 3).is_err());
        assert_eq!(parse_ordered("3-", 5).unwrap(), vec![3, 4, 5]);
        assert_eq!(parse_ordered("-2", 5).unwrap(), vec![1, 2]);
        assert_eq!(parse_ordered("1 3  5", 5).unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_ordered("2 \u{2013} 4; 1", 5).unwrap(), vec![2, 3, 4, 1]);
        assert_eq!(parse_ordered("1 - 4 : even", 5).unwrap(), vec![2, 4]);
        assert_eq!(parse_ordered("1, 2", 5).unwrap(), vec![1, 2]);
        assert_eq!(parse_ordered(" 2 , 3 ", 5).unwrap(), vec![2, 3]);
        assert!(parse_ordered("-", 3).is_err());
    }
}
