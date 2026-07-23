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

    write_clientsettings(install_dir, &root)
}

/// Write the whole settings object back, pretty-printed, creating the dir/file.
fn write_clientsettings(install_dir: &Path, root: &Value) -> Result<(), String> {
    std::fs::create_dir_all(install_dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(root).map_err(|e| e.to_string())?;
    std::fs::write(clientsettings_path(install_dir), text).map_err(|e| e.to_string())
}

/// Add (or refresh) a server in the install's in-game multiplayer list. VS stores
/// each entry as a `name,host:port,password` string under
/// `stringListSettings.multiplayerservers`, splitting on commas - so the name is
/// kept comma-free, and any existing entry for the same address is replaced
/// rather than duplicated. Merges non-destructively with the rest of the file.
pub fn add_multiplayer_server(
    install_dir: &Path,
    name: &str,
    address: &str,
    password: Option<&str>,
) -> Result<(), String> {
    let address = address.trim();
    if address.is_empty() {
        return Err("server address is empty".into());
    }
    let mut root = read_clientsettings(install_dir);
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let sls = obj.entry("stringListSettings").or_insert_with(|| json!({}));
    if !sls.is_object() {
        *sls = json!({});
    }
    let sls = sls.as_object_mut().unwrap();
    let list = sls.entry("multiplayerservers").or_insert_with(|| json!([]));
    if !list.is_array() {
        *list = json!([]);
    }
    let arr = list.as_array_mut().unwrap();

    // Drop any existing entry whose address field (the 2nd CSV column) matches.
    arr.retain(|v| {
        v.as_str()
            .and_then(|s| s.splitn(3, ',').nth(1))
            .map(|a| a != address)
            .unwrap_or(true)
    });

    let clean_name = name.replace(',', " ");
    let clean_name = clean_name.trim();
    let clean_name = if clean_name.is_empty() { address } else { clean_name };
    arr.push(json!(format!("{},{},{}", clean_name, address, password.unwrap_or(""))));

    write_clientsettings(install_dir, &root)
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
