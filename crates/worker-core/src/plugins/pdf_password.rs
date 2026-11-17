//! PDF password: protect a PDF with a password (AES-256 by default) and optional
//! limits on printing, copying and editing, or remove a password the user knows.

use super::{file_size, Analysis, Ctx, Plugin};
use crate::common::pdf;
use contract::{opt, t, FileError, FileRef, Request};
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::{EncryptionState, EncryptionVersion, Permissions};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct PdfPassword;

impl Plugin for PdfPassword {
    fn id(&self) -> &'static str {
        "pdf-password"
    }

    fn manifest(&self) -> Value {
        let protect = json!({ "field": "action", "eq": "protect" });
        let limits = json!({ "all": [ { "field": "action", "eq": "protect" }, { "field": "restrict", "eq": true } ] });
        json!({
            "name": t("Parolă PDF", "PDF password"),
            "summary": t("Pune sau scoate parola", "Lock or unlock"),
            "keywords": { "ro": ["parolă", "protejează", "criptează", "deblochează", "scoate parola"], "en": ["password", "protect", "encrypt", "unlock", "remove password"] },
            "category": "pdf",
            "icon": "key",
            "input": { "extensions": ["pdf"], "multiple": true, "folders": true, "mode": "perFile" },
            "output": { "kind": "perInput", "extensions": ["pdf"], "replaceOriginalAllowed": false },
            "analysis": { "fast": ["needsPassword", "pageCount", "isEncrypted", "isSigned"] },
            "ui": { "preview": "none", "estimate": false },
            "resources": { "class": "light", "diskFactor": 1.2 },
            "presets": [],
            "groups": [ { "id": "more", "label": t("Detalii", "Details"), "order": 1 } ],
            "fields": [
                { "key": "action", "type": "select", "label": t("Ce faci", "What to do"), "default": "protect", "display": "chips",
                  "options": [ opt("protect", "Pune o parolă", "Add a password"), opt("remove", "Scoate parola", "Remove the password") ] },
                { "key": "openPassword", "type": "password", "label": t("Parola pentru deschidere", "Password to open it"), "default": "",
                  "help": t("Fără ea, fișierul nu se poate deschide.", "Without it, the file can't be opened."), "showIf": protect },
                { "key": "restrict", "type": "toggle", "default": false, "label": t("Limitează tipărirea și copierea", "Limit printing and copying"),
                  "showIf": protect },
                { "key": "allowPrint", "type": "select", "label": t("Tipărire", "Printing"), "default": "full",
                  "options": [ opt("full", "Permisă", "Allowed"), opt("low", "Doar la calitate slabă", "Low quality only"), opt("none", "Interzisă", "Not allowed") ],
                  "showIf": limits },
                { "key": "allowCopy", "type": "toggle", "default": false, "label": t("Permite copierea textului și a imaginilor", "Allow copying text and images"),
                  "showIf": limits },
                { "key": "allowModify", "type": "select", "label": t("Modificări", "Changes"), "default": "none",
                  "options": [ opt("all", "Permise", "Allowed"), opt("annotate", "Doar comentarii și formulare", "Comments and forms only"),
                               opt("form", "Doar completarea formularelor", "Filling in forms only"), opt("assembly", "Doar ordinea paginilor", "Page order only"),
                               opt("none", "Interzise", "Not allowed") ],
                  "showIf": limits },
                { "key": "ownerPassword", "type": "password", "advanced": true, "group": "more", "default": "",
                  "label": t("Parola pentru schimbarea limitelor", "Password to change the limits"),
                  "help": t("Dacă rămâne goală, pun una aleatorie: limitele nu se mai pot schimba.", "If left empty, a random one is used: the limits can't be changed later."),
                  "showIf": limits },
                { "key": "encryption", "type": "select", "advanced": true, "group": "more", "default": "aes256",
                  "label": t("Criptare", "Encryption"),
                  "options": [ opt("aes256", "AES 256 (puternică)", "AES 256 (strong)"), opt("aes128", "AES 128 (cititoare vechi)", "AES 128 (old readers)") ],
                  "showIf": protect }
            ],
            "rules": [
                { "id": "no_recovery", "severity": "info", "when": { "all": [ protect, { "field": "openPassword", "ne": "" } ] },
                  "message": t("Dacă uiți parola, fișierul nu mai poate fi deschis. Originalul rămâne neschimbat.",
                               "If you forget the password, the file can't be opened. The original stays as it is.") },
                { "id": "limits_soft", "severity": "info", "when": limits,
                  "message": t("Limitele de tipărire și copiere nu sunt o protecție reală: unele programe le ignoră. Doar parola de deschidere protejează conținutul.",
                               "Print and copy limits aren't real protection: some programs ignore them. Only the open password protects the content.") },
                { "id": "nothing_to_do", "severity": "error", "fields": ["openPassword"],
                  "when": { "all": [ protect, { "field": "openPassword", "eq": "" }, { "not": { "field": "restrict", "eq": true } } ] },
                  "message": t("Scrie o parolă sau alege limite.", "Type a password or choose some limits.") },
                { "id": "not_encrypted", "severity": "warning",
                  "when": { "all": [ { "field": "action", "eq": "remove" }, { "file": "isEncrypted", "eq": false } ] },
                  "message": t("Fișierul nu are parolă.", "The file has no password.") },
                { "id": "signature_lost", "severity": "confirm", "when": { "file": "isSigned", "eq": true },
                  "message": t("Semnăturile digitale nu vor mai fi valide.", "Digital signatures won't be valid anymore.") }
            ],
            "outputBlock": { "defaults": { "nameTemplate": t("{name}_parola", "{name}_password") } },
            "errors": {
                "password_empty": t("Scrie o parolă sau alege limite.", "Type a password or choose some limits."),
                "not_encrypted": t("Fișierul nu are parolă.", "The file has no password."),
                "password_needs_aes256": t("Parola are litere pe care criptarea AES-128 nu le poate păstra (de exemplu ă, ș, ț). Alege AES-256 sau o parolă fără ele.",
                                           "The password has letters AES-128 can't keep (such as ă, ș, ț). Choose AES-256 or a password without them.")
            }
        })
    }

    fn analyze(&self, f: &FileRef, req: &Request) -> Analysis {
        let (status, props, code) = pdf::analyze(f, req);
        Analysis { status, props, warnings: vec![], code }
    }

    fn run_file(&self, ctx: &Ctx, f: &FileRef) -> Result<Value, FileError> {
        let req = ctx.req;
        let in_bytes = file_size(f.input_path());
        ctx.progress(&f.id, 0.0, "read", 1, 2);
        let mut doc = pdf::open_file(f, req)?;
        let was = doc.was_encrypted();
        // Decrypted on load: write it plain unless protecting again.
        doc.encryption_state = None;
        ctx.check_cancel()?;
        ctx.progress(&f.id, 0.4, "write", 2, 2);
        let action = req.str_or("action", "protect");
        if action == "remove" {
            if !was {
                return Err(FileError::new("not_encrypted"));
            }
        } else {
            let user = req.str_or("openPassword", "");
            let restrict = req.bool_or("restrict", false);
            if user.is_empty() && !restrict {
                return Err(FileError::new("password_empty"));
            }
            // AES-128 stores passwords in an old Latin character set: letters outside it
            // (ă ș ț, Greek, Cyrillic…) are read differently by each program.
            let kind = req.str_or("encryption", "aes256");
            let owner_given_check = req.str_or("ownerPassword", "");
            if kind == "aes128" && (user.chars().any(|c| c as u32 > 0xFF) || owner_given_check.chars().any(|c| c as u32 > 0xFF)) {
                return Err(FileError::new("password_needs_aes256"));
            }
            let perms = if restrict { permissions(req) } else { Permissions::all() };
            let owner_given = req.str_or("ownerPassword", "");
            // Without restrictions the owner password matters little; never leave it empty or equal.
            let owner = if !owner_given.is_empty() && owner_given != user { owner_given.to_string() } else { random_password()? };
            ensure_id(&mut doc)?;
            let state = encryption(&doc, req.str_or("encryption", "aes256"), &owner, user, perms)?;
            doc.encrypt(&state).map_err(FileError::engine)?;
        }
        let path = ctx.file_dir(&f.id)?.join("result.pdf");
        pdf::save(&mut doc, &path, false)?;
        ctx.progress(&f.id, 1.0, "write", 2, 2);
        ctx.emit_output(&f.id, &path, "pdf", json!({}), in_bytes);
        Ok(Value::Null)
    }
}

fn permissions(req: &Request) -> Permissions {
    let mut p = Permissions::COPYABLE_FOR_ACCESSIBILITY;
    match req.str_or("allowPrint", "full") {
        "full" => p |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY,
        "low" => p |= Permissions::PRINTABLE,
        _ => {}
    }
    if req.bool_or("allowCopy", false) {
        p |= Permissions::COPYABLE;
    }
    p |= match req.str_or("allowModify", "none") {
        "all" => Permissions::MODIFIABLE | Permissions::ANNOTABLE | Permissions::FILLABLE | Permissions::ASSEMBLABLE,
        "annotate" => Permissions::ANNOTABLE | Permissions::FILLABLE,
        "form" => Permissions::FILLABLE,
        "assembly" => Permissions::ASSEMBLABLE,
        _ => Permissions::empty(),
    };
    p
}

fn encryption(doc: &lopdf::Document, kind: &str, owner: &str, user: &str, permissions: Permissions) -> Result<EncryptionState, FileError> {
    let version = if kind == "aes128" {
        let cf: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
        EncryptionVersion::V4 {
            document: doc,
            encrypt_metadata: true,
            crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), cf)]),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: user,
            permissions,
        }
    } else {
        let cf: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(FileError::engine)?;
        return EncryptionState::try_from(EncryptionVersion::V5 {
            encrypt_metadata: true,
            crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), cf)]),
            file_encryption_key: &key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: user,
            permissions,
        })
        .map_err(FileError::engine);
    };
    EncryptionState::try_from(version).map_err(FileError::engine)
}

/// AES-128 derives its key from the document ID; some files have none.
fn ensure_id(doc: &mut lopdf::Document) -> Result<(), FileError> {
    let ok = doc.trailer.get(b"ID").and_then(lopdf::Object::as_array).map(|a| !a.is_empty()).unwrap_or(false);
    if !ok {
        let mut id = vec![0u8; 16];
        getrandom::fill(&mut id).map_err(FileError::engine)?;
        let s = lopdf::Object::String(id, lopdf::StringFormat::Hexadecimal);
        doc.trailer.set("ID", lopdf::Object::Array(vec![s.clone(), s]));
    }
    Ok(())
}

/// 24 random letters and digits, for an owner password nobody needs to know.
fn random_password() -> Result<String, FileError> {
    const SET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(FileError::engine)?;
    Ok(bytes.iter().map(|b| SET[*b as usize % SET.len()] as char).collect())
}
