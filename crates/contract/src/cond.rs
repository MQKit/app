//! Condition expressions (contract section 7.1), the same rules as the UI's `cond.ts`.

use serde_json::{Map, Value};

pub struct Ctx<'a> {
    pub values: &'a Map<String, Value>,
    /// Analysis props of the files in the batch.
    pub files: &'a [Map<String, Value>],
    pub env: &'a Map<String, Value>,
}

fn get(obj: &Map<String, Value>, path: &str) -> Option<Value> {
    if let Some(v) = obj.get(path) {
        return Some(v.clone());
    }
    let mut cur = Value::Object(obj.clone());
    for part in path.split('.') {
        if part == "count" {
            if let Value::Array(a) = &cur {
                return Some(Value::from(a.len()));
            }
        }
        cur = cur.get(part)?.clone();
    }
    Some(cur)
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn compare(v: Option<Value>, c: &Map<String, Value>) -> bool {
    if let Some(k) = c.get("known") {
        return v.is_some() == k.as_bool().unwrap_or(true);
    }
    let Some(v) = v else { return false };
    if let Some(e) = c.get("eq") {
        return &v == e || (num(&v).is_some() && num(&v) == num(e));
    }
    if let Some(e) = c.get("ne") {
        return &v != e;
    }
    if let Some(Value::Array(a)) = c.get("in") {
        return a.contains(&v);
    }
    if let Some(Value::Array(a)) = c.get("notIn") {
        return !a.contains(&v);
    }
    let n = num(&v);
    for (op, f) in [("gt", (|a: f64, b: f64| a > b) as fn(f64, f64) -> bool), ("gte", |a, b| a >= b), ("lt", |a, b| a < b), ("lte", |a, b| a <= b)] {
        if let Some(b) = c.get(op).and_then(num) {
            return n.map(|a| f(a, b)).unwrap_or(false);
        }
    }
    if let Some(x) = c.get("contains") {
        return match &v {
            Value::Array(a) => a.contains(x),
            Value::String(s) => x.as_str().map(|t| s.contains(t)).unwrap_or(false),
            _ => false,
        };
    }
    if let Some(e) = c.get("empty") {
        let is_empty = match &v {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        };
        return is_empty == e.as_bool().unwrap_or(true);
    }
    false
}

/// `None` (no condition) is true.
pub fn eval(c: Option<&Value>, ctx: &Ctx) -> bool {
    let Some(Value::Object(c)) = c else { return true };
    if let Some(Value::Array(a)) = c.get("all") {
        return a.iter().all(|x| eval(Some(x), ctx));
    }
    if let Some(Value::Array(a)) = c.get("any") {
        return a.iter().any(|x| eval(Some(x), ctx));
    }
    if let Some(x) = c.get("not") {
        return !eval(Some(x), ctx);
    }
    if let Some(k) = c.get("field").and_then(Value::as_str) {
        return compare(ctx.values.get(k).cloned(), c);
    }
    if let Some(k) = c.get("env").and_then(Value::as_str) {
        return compare(ctx.env.get(k).cloned(), c);
    }
    if let Some(k) = c.get("file").and_then(Value::as_str) {
        let results: Vec<bool> = ctx.files.iter().map(|f| compare(get(f, k), c)).collect();
        return match c.get("quant").and_then(Value::as_str).unwrap_or("any") {
            "all" => !results.is_empty() && results.iter().all(|x| *x),
            "none" => !results.iter().any(|x| *x),
            _ => results.iter().any(|x| *x),
        };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn basics() {
        let values = json!({"engine": "auto", "q": 80}).as_object().unwrap().clone();
        let env = json!({"office.word": "ok"}).as_object().unwrap().clone();
        let files = vec![json!({"hasForms": true, "audioTracks": [1, 2]}).as_object().unwrap().clone()];
        let ctx = Ctx { values: &values, files: &files, env: &env };
        assert!(eval(Some(&json!({"all": [{"env": "office.word", "eq": "ok"}, {"field": "engine", "ne": "libreoffice"}]})), &ctx));
        assert!(eval(Some(&json!({"field": "q", "gte": 80})), &ctx));
        assert!(eval(Some(&json!({"file": "audioTracks.count", "gt": 1})), &ctx));
        assert!(!eval(Some(&json!({"file": "hasForms", "eq": false})), &ctx));
        assert!(eval(None, &ctx));
    }

    #[test]
    fn word_is_skipped_when_any_pdf_is_encrypted() {
        let values = json!({"engine": "auto"}).as_object().unwrap().clone();
        let env = json!({"office.word": "ok"}).as_object().unwrap().clone();
        let word = json!({"all": [{"all": [{"env": "office.word", "eq": "ok"}, {"field": "engine", "ne": "pdf2docx"}]},
                                  {"not": {"file": "isEncrypted", "eq": true}}]});
        let plain = json!({"isEncrypted": false}).as_object().unwrap().clone();
        let locked = json!({"isEncrypted": true}).as_object().unwrap().clone();
        let one = vec![plain.clone()];
        assert!(eval(Some(&word), &Ctx { values: &values, files: &one, env: &env }));
        let mixed = vec![plain, locked];
        assert!(!eval(Some(&word), &Ctx { values: &values, files: &mixed, env: &env }));
    }
}
