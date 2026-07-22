//! Reading and writing an installation's `clientsettings.json`.
//!
//! The auth fields live under `stringSettings` with lowercase keys (confirmed
//! against the game's own canonical file). We merge non-destructively so we
//! never clobber unrelated settings.
//!
//! `read_back_session` is the half neither incumbent implements: after the game
//! exits it may have written a NEWER (rotated) session into this file. We read
//! that back so our stored account stays current instead of going stale.

use crate::models::Account;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn clientsettings_path(install_dir: &Path) -> PathBuf {
    install_dir.join("clientsettings.json")
}

/// Parse the install's clientsettings.json, or an empty object if absent/invalid.
pub fn read_clientsettings(install_dir: &Path) -> Value {
    std::fs::read_to_string(clientsettings_path(install_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// Merge the account's credentials into `stringSettings`, preserving every other
/// key already in the file. Creates the file/dir if missing.
pub fn stamp_session(install_dir: &Path, account: &Account) -> Result<(), String> {
    let mut root = read_clientsettings(install_dir);
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let ss = obj
        .entry("stringSettings")
        .or_insert_with(|| json!({}));
    if !ss.is_object() {
        *ss = json!({});
    }
    let ss = ss.as_object_mut().unwrap();
    ss.insert("playeruid".into(), json!(account.uid));
    ss.insert("playername".into(), json!(account.playername));
    ss.insert("useremail".into(), json!(account.email));
    ss.insert("sessionkey".into(), json!(account.sessionkey));
    ss.insert("sessionsignature".into(), json!(account.sessionsignature));
    if let Some(mpt) = &account.mptoken {
        ss.insert("mptoken".into(), json!(mpt));
    }

    std::fs::create_dir_all(install_dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(clientsettings_path(install_dir), text).map_err(|e| e.to_string())
}

/// The session currently written in the install's clientsettings.json, if any.
/// Returns `(uid, sessionkey, sessionsignature, playername)`.
pub fn read_back_session(install_dir: &Path) -> Option<(String, String, String, String)> {
    let root = read_clientsettings(install_dir);
    let ss = root.get("stringSettings")?;
    let get = |k: &str| ss.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (uid, key, sig) = (get("playeruid")?, get("sessionkey")?, get("sessionsignature")?);
    if key.is_empty() || sig.is_empty() {
        return None;
    }
    Some((uid, key, sig, get("playername").unwrap_or_default()))
}
