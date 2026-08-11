//! Translocator's own installation store.
//!
//! Each installation folder (its `--dataPath`) carries a `translocator.json`
//! holding the metadata VS Launcher and StoryForge keep: display name, pinned
//! game version, launch params, env vars, backup preferences, and playtime.
//! Existing folders are ADOPTED in place on first read - we write our metadata
//! file alongside the game's own files, so nothing is moved, lost, or
//! duplicated. This is what turns Translocator from a reader of someone else's
//! installations into the owner of its own.

use crate::mods;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

const META_FILE: &str = "translocator.json";

fn default_compression() -> u8 {
    6
}

fn default_backups_limit() -> u8 {
    5
}

#[derive(Serialize, Deserialize, Clone)]
pub struct InstallationMeta {
    pub name: String,
    /// Pinned VS version. Metadata in Phase A (used for update-compatibility);
    /// Phase B ties it to a downloaded game version for launch.
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub start_params: String,
    /// "K=V, K2=V2" - same freeform shape VS Launcher uses.
    #[serde(default)]
    pub env_vars: String,
    /// Take a backup before launching.
    #[serde(default)]
    pub auto_backup: bool,
    #[serde(default = "default_compression")]
    pub compression: u8,
    #[serde(default = "default_backups_limit")]
    pub backups_limit: u8,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub favorite: bool,
    /// ms since epoch; 0 = never played.
    #[serde(default)]
    pub last_played: u64,
    /// total seconds played through Translocator.
    #[serde(default)]
    pub total_time_played: u64,

    // ---- pack-managed freeze (docs/modpack-manifest.md step 7) ----
    // All default/skipped so a personal install's translocator.json is unchanged.
    /// `"<pack.id>@<pack.version>"` while this install is pack-managed; empty
    /// otherwise. While set, the per-mod update manager is disabled and the only
    /// update path is a whole-pack update.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub managed_by: String,
    /// The managing pack's `strict` flag: true = the manifest IS the Mods
    /// folder, extra mod installs are refused and updates sweep foreign zips.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pack_strict: bool,
    /// Zip filenames the pack placed; what update reconciliation may touch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pack_owned_files: Vec<String>,
    /// Canonical hash of each override as applied (semantic-canonical for JSON),
    /// keyed by the override's relative path. The user-edit detector.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub override_hashes: BTreeMap<String, String>,
    /// The user's optional-mod choices, keyed by modidstr; preserved across
    /// pack updates. Missing key = the manifest's `required` default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub optional_choices: BTreeMap<String, bool>,
}

impl Default for InstallationMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            start_params: String::new(),
            env_vars: String::new(),
            auto_backup: false,
            compression: 6,
            backups_limit: 5,
            icon: String::new(),
            favorite: false,
            last_played: 0,
            total_time_played: 0,
            managed_by: String::new(),
            pack_strict: false,
            pack_owned_files: Vec::new(),
            override_hashes: BTreeMap::new(),
            optional_choices: BTreeMap::new(),
        }
    }
}

fn folder_display_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().replace(['-', '_'], " ").trim().to_string())
        .unwrap_or_default()
}

pub fn read_meta(dir: &Path) -> Option<InstallationMeta> {
    let text = std::fs::read_to_string(dir.join(META_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_meta(dir: &Path, meta: &InstallationMeta) -> Result<(), String> {
    let text = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(META_FILE), text).map_err(|e| e.to_string())
}

/// Read the install's metadata, or adopt the folder in place by writing a
/// default derived from the folder name + the launcher's default version.
pub fn import_or_read(dir: &Path, default_version: &str) -> InstallationMeta {
    if let Some(m) = read_meta(dir) {
        return m;
    }
    let meta = InstallationMeta {
        name: folder_display_name(dir),
        version: default_version.to_string(),
        ..Default::default()
    };
    let _ = write_meta(dir, &meta); // adopt in place
    meta
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record a completed play session: stamp last_played and add to playtime.
pub fn record_play(dir: &Path, elapsed_secs: u64) {
    let mut meta = read_meta(dir).unwrap_or_else(|| InstallationMeta {
        name: folder_display_name(dir),
        ..Default::default()
    });
    meta.last_played = now_ms();
    meta.total_time_played += elapsed_secs;
    let _ = write_meta(dir, &meta);
}

#[derive(Serialize)]
pub struct InstallationCard {
    pub path: String,
    pub meta: InstallationMeta,
    pub mod_count: usize,
    pub has_session: bool,
}

/// A subfolder is treated as an installation only if it looks like a VS data
/// folder. This keeps `list` from adopting (and writing metadata into) unrelated
/// directories if the installations path is ever misconfigured.
pub(crate) fn looks_like_install(dir: &Path) -> bool {
    dir.join("Mods").is_dir()
        || dir.join("clientsettings.json").is_file()
        || dir.join("Saves").is_dir()
        || dir.join(META_FILE).is_file()
}

/// Every installation under `installations_dir`, adopting any not yet tracked.
/// Sorted favorites-first, then by name.
pub fn list(installations_dir: &Path, default_version: &str) -> Result<Vec<InstallationCard>, String> {
    let mut cards = Vec::new();
    // A not-yet-created folder is normal on first run or after relocating the
    // path; treat it as "no installations" rather than an error.
    if !installations_dir.exists() {
        return Ok(cards);
    }
    for entry in std::fs::read_dir(installations_dir).map_err(|e| format!("read_dir failed: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let path = entry.path();
            // Dot-prefixed folders are ours (delete tombstones, pack stages),
            // never installations to adopt.
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if !looks_like_install(&path) {
                continue;
            }
            let meta = import_or_read(&path, default_version);
            let mod_count = mods::list_mod_files(&path).len();
            let has_session = crate::session::read_back_session(&path).is_some();
            cards.push(InstallationCard {
                path: path.to_string_lossy().into_owned(),
                meta,
                mod_count,
                has_session,
            });
        }
    }
    cards.sort_by(|a, b| {
        b.meta
            .favorite
            .cmp(&a.meta.favorite)
            .then(a.meta.name.to_lowercase().cmp(&b.meta.name.to_lowercase()))
    });
    Ok(cards)
}

pub(crate) fn slugify(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    // collapse repeated hyphens
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    if out.is_empty() {
        "installation".into()
    } else {
        out
    }
}

/// Auth/session fields that must NEVER be copied between installs (copying a
/// stale session is the exact bug that breaks the incumbents' carryover), and
/// that must never leave this machine inside a backup zip.
const AUTH_KEYS: &[&str] = &[
    "sessionkey",
    "sessionsignature",
    "mptoken",
    "useremail",
    "playeruid",
    "playername",
    "entitlements",
    "hostgameserver",
];

/// Remove the auth/session fields from a parsed clientsettings.json. Returns
/// true if anything was actually removed. Shared by install seeding, backup
/// sanitising and the logout scrub, so the three can never drift apart.
pub fn strip_auth(v: &mut serde_json::Value) -> bool {
    let Some(ss) = v.get_mut("stringSettings").and_then(|s| s.as_object_mut()) else {
        return false;
    };
    let mut hit = false;
    for k in AUTH_KEYS {
        hit |= ss.remove(*k).is_some();
    }
    hit
}

/// Blank the password column of every saved multiplayer server. VS stores each
/// entry as `name,host:port,password` under `stringListSettings
/// .multiplayerservers` (see session::add_multiplayer_server), so this keeps
/// the server list intact and drops only the credential. Returns true if any
/// entry changed.
///
/// Used on the backup path only: a backup zip is made to be moved somewhere
/// else, and these are real passwords to other people's servers. The launcher
/// keeps its own sealed copy, so connecting through Translocator writes them
/// back on the next join.
pub fn strip_server_passwords(v: &mut serde_json::Value) -> bool {
    let Some(list) = v
        .get_mut("stringListSettings")
        .and_then(|s| s.as_object_mut())
        .and_then(|o| o.get_mut("multiplayerservers"))
        .and_then(|l| l.as_array_mut())
    else {
        return false;
    };
    let mut hit = false;
    for entry in list.iter_mut() {
        let Some(s) = entry.as_str() else { continue };
        // Only the first two commas are structural; anything after them is the
        // password, however many commas the user managed to put in it.
        let mut parts = s.splitn(3, ',');
        let (Some(name), Some(addr)) = (parts.next(), parts.next()) else {
            continue;
        };
        let pw = parts.next().unwrap_or("");
        if pw.is_empty() {
            continue;
        }
        *entry = serde_json::json!(format!("{name},{addr},"));
        hit = true;
    }
    hit
}

/// Read an install's clientsettings.json, strip the auth/session fields, and
/// write it back. `Ok(false)` when there was no file or nothing to remove.
/// This is what logout uses so signing out clears the live session key the game
/// left in every install, not just our own account.dat.
pub fn scrub_session(install_dir: &Path) -> Result<bool, String> {
    let path = install_dir.join("clientsettings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        // A corrupt settings file is the game's problem, not ours; leave it be
        // rather than rewriting something we could not parse.
        Err(_) => return Ok(false),
    };
    if !strip_auth(&mut v) {
        return Ok(false);
    }
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Every install folder's clientsettings.json under `installations_dir`, with
/// the auth fields removed. Returns how many files were actually rewritten.
pub fn scrub_all_sessions(installations_dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(installations_dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| scrub_session(&e.path()).unwrap_or(false))
        .count()
}

/// Copy the game's client settings (keybinds, graphics, audio, GUI scale, ...)
/// from `source_dir` into `dest_dir`, STRIPPING the auth/session fields so a new
/// install inherits your dialed-in settings but gets its own freshly stamped
/// login at launch. Returns true if a source clientsettings.json was found.
pub fn seed_clientsettings(source_dir: &Path, dest_dir: &Path) -> Result<bool, String> {
    let text = match std::fs::read_to_string(source_dir.join("clientsettings.json")) {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let mut v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    strip_auth(&mut v);
    // The mod paths must belong to the DESTINATION install. The relative
    // "Mods" entry is the game folder's bundled system mods; the absolute
    // entry is this install's own Mods dir. Copying the source's absolute
    // path verbatim was the bug that silently pointed a new install at
    // another install's mod set.
    if let Some(sls) = v
        .get_mut("stringListSettings")
        .and_then(|s| s.as_object_mut())
    {
        sls.insert(
            "modPaths".to_string(),
            serde_json::json!([
                "Mods",
                dest_dir.join("Mods").to_string_lossy().into_owned(),
            ]),
        );
    }
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(dest_dir.join("clientsettings.json"), out).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Create a fresh installation: a new isolated dataPath folder (with a Mods/
/// dir) and its metadata pinned to `version`. Returns the new folder path.
pub fn create(installations_dir: &Path, name: &str, version: &str) -> Result<String, String> {
    let dir = installations_dir.join(slugify(name));
    if dir.exists() {
        return Err(format!("a folder for '{name}' already exists"));
    }
    std::fs::create_dir_all(dir.join("Mods")).map_err(|e| e.to_string())?;
    let meta = InstallationMeta {
        name: name.to_string(),
        version: version.to_string(),
        ..Default::default()
    };
    write_meta(&dir, &meta)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Delete an installation folder. The UI guards this with a confirm.
///
/// A Quire-sized install is gigabytes, so removal takes many seconds. The
/// folder is first RENAMED to a dot-prefixed tombstone (a same-volume metadata
/// op, effectively instant), so it vanishes from the installations listing
/// immediately; the real removal then grinds through the tombstone. If the
/// rename fails (an open file locks the dir on Windows), fall back to deleting
/// in place.
pub fn delete(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tomb = path.with_file_name(format!(".deleting-{name}"));
    let target = if !tomb.exists() && std::fs::rename(path, &tomb).is_ok() {
        tomb
    } else {
        path.to_path_buf()
    };
    std::fs::remove_dir_all(&target).map_err(|e| format!("delete failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings() -> serde_json::Value {
        json!({
            "stringSettings": {
                "sessionkey": "live-key",
                "sessionsignature": "sig",
                "mptoken": "mp",
                "useremail": "someone@example.com",
                "playeruid": "uid",
                "playername": "Venah",
                "guiScale": "1.0"
            },
            "stringListSettings": {
                // `name,host:port,password`, per session::add_multiplayer_server,
                // which strips commas from the name. So only the password can
                // contain one, and everything past the second comma is password.
                "multiplayerservers": [
                    "The Quire,quire.example:42420,hunter2",
                    "No password,other.example:42420,",
                    "Third,third.example:42420,pw,with,commas"
                ],
                "modPaths": ["Mods"]
            }
        })
    }

    #[test]
    fn strip_auth_removes_every_credential_and_keeps_the_rest() {
        let mut v = settings();
        assert!(strip_auth(&mut v));
        let ss = v["stringSettings"].as_object().unwrap();
        for k in AUTH_KEYS {
            assert!(!ss.contains_key(*k), "{k} survived the strip");
        }
        // Settings the user cares about are untouched.
        assert_eq!(ss["guiScale"], json!("1.0"));
        // Nothing left to do the second time.
        assert!(!strip_auth(&mut v));
    }

    #[test]
    fn server_passwords_are_blanked_but_the_list_survives() {
        let mut v = settings();
        assert!(strip_server_passwords(&mut v));
        let list = v["stringListSettings"]["multiplayerservers"].as_array().unwrap();
        assert_eq!(list[0], json!("The Quire,quire.example:42420,"));
        assert_eq!(list[1], json!("No password,other.example:42420,"));
        // A password containing commas is still only ONE field: everything
        // after the second comma goes, or the tail leaks.
        assert_eq!(list[2], json!("Third,third.example:42420,"));
        assert!(!strip_server_passwords(&mut v));
    }

    #[test]
    fn a_sanitized_file_contains_no_secret_substring() {
        let mut v = settings();
        strip_auth(&mut v);
        strip_server_passwords(&mut v);
        let text = serde_json::to_string(&v).unwrap();
        for secret in ["live-key", "someone@example.com", "hunter2", "pw,with,commas"] {
            assert!(!text.contains(secret), "{secret} still present in {text}");
        }
    }

    #[test]
    fn missing_sections_are_not_an_error() {
        let mut empty = json!({});
        assert!(!strip_auth(&mut empty));
        assert!(!strip_server_passwords(&mut empty));
    }
}
