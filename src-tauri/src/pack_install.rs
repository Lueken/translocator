//! The install-pack flow: turn a Hub manifest into a pack-managed installation.
//!
//! Canonical spec: `docs/modpack-manifest.md` (Install / resolution algorithm).
//! The invariant everything here serves: `Mods/` is only ever touched by a FULLY
//! verified set. Every selected mod downloads to a stage dir and passes its
//! SHA-256 + zip check before a single file is placed; any failure discards the
//! stage and, on a fresh install, the half-created folder too. A half-updated
//! pack matches neither version, which is the exact mismatch the freeze exists
//! to prevent.

use crate::manifest::{Manifest, ManifestMod};
use crate::{installations, mods, optimum, session, signing, versions};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

/// Dotted-numeric version compare ("0.2.0" style; non-numeric segments compare
/// as 0). Returns true when `have` >= `need`.
fn version_at_least(have: &str, need: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split(['.', '-'])
            .map(|s| s.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (h, n) = (parse(have), parse(need));
    for i in 0..h.len().max(n.len()) {
        let a = h.get(i).copied().unwrap_or(0);
        let b = n.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    true
}

/// A pack id we are willing to put in a filesystem path.
///
/// The Hub validates this too, but the Hub is the party we are defending
/// against here: the manifest arrives over the network and the id lands in
/// `.pack-stage-{id}`, a directory that gets `remove_dir_all`d. A hostile or
/// compromised Hub serving `../../..` would have deleted whatever that
/// resolved to. Same rule both ends: lowercase, digits, hyphen, 2 to 64.
pub fn safe_pack_id(id: &str) -> bool {
    let n = id.len();
    if !(2..=64).contains(&n) {
        return false;
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap_or(' ');
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `"<id>@<version>"` for the freeze; split back with [`parse_managed`].
pub fn managed_key(pack_id: &str, version: &str) -> String {
    format!("{pack_id}@{version}")
}

/// The `(pack_id, version)` inside a `managed_by` value, if well-formed.
pub fn parse_managed(managed_by: &str) -> Option<(&str, &str)> {
    let at = managed_by.rfind('@')?;
    let (id, ver) = (&managed_by[..at], &managed_by[at + 1..]);
    if id.is_empty() || ver.is_empty() {
        None
    } else {
        Some((id, ver))
    }
}

/// The canonical hash of an override's content: for `.json` paths a sha256 over
/// RFC 8785-style canonical JSON (mods re-serialize their configs every launch,
/// reordering keys, so a byte hash would read "user-edited" after one launch);
/// for everything else, and for JSON that fails to parse, a plain byte hash.
/// The SAME function runs at apply time and at compare time, so a parse-failure
/// fallback stays consistent with itself.
pub fn override_canonical_hash(rel_path: &str, bytes: &[u8]) -> String {
    if rel_path.to_ascii_lowercase().ends_with(".json") {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
            let mut canon = String::new();
            signing::canonical_json(&v, &mut canon);
            return hex_sha256(canon.as_bytes());
        }
    }
    hex_sha256(bytes)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Resolve an override's relative path inside the install, refusing absolute
/// paths, `..`, and drive/UNC forms (spec: Validation rules).
fn safe_override_path(install_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let bad = rel.is_empty()
        || rel.contains("..")
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains(':');
    if bad {
        return Err(format!("override path '{rel}' is not a safe relative path"));
    }
    Ok(install_dir.join(rel))
}

/// Which manifest entries actually install on this client: never server-only
/// mods, and optional mods only when the user opted in.
pub fn select_mods<'m>(
    manifest: &'m Manifest,
    optional_choices: &BTreeMap<String, bool>,
) -> Vec<&'m ManifestMod> {
    manifest
        .mods
        .iter()
        .filter(|m| m.side != "server")
        .filter(|m| m.required || optional_choices.get(&m.modidstr).copied().unwrap_or(false))
        .collect()
}

fn emit_progress(
    app: &AppHandle,
    pack_id: &str,
    phase: &str,
    detail: &str,
    done: usize,
    total: usize,
    received: u64,
    bytes_total: u64,
) {
    let _ = app.emit(
        "pack-progress",
        serde_json::json!({
            "pack_id": pack_id,
            "phase": phase,
            "detail": detail,
            "done": done,
            "total": total,
            "received": received,
            "bytes_total": bytes_total,
        }),
    );
}

/// Download ONE pinned mod into the stage dir, hashing while streaming, and
/// verify it against the manifest pin before it earns a real filename. Returns
/// the staged zip's final basename.
async fn stage_one(
    app: &AppHandle,
    client: &reqwest::Client,
    stage_dir: &Path,
    m: &ManifestMod,
    pack_id: &str,
    done: usize,
    total: usize,
) -> Result<String, String> {
    use futures::StreamExt;

    let url = format!("https://mods.vintagestory.at/download?fileid={}", m.fileid);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("{}: download failed: {e}", m.name))?;
    if !resp.status().is_success() {
        return Err(format!("{}: ModDB returned {} for fileid {}", m.name, resp.status(), m.fileid));
    }

    // Prefer ModDB's own filename (Content-Disposition) so the staged zip is
    // named what everyone recognizes; anything odd falls back to a constructed
    // name. Either way it can only ever be a bare .zip basename.
    let raw_name = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split("filename=").nth(1))
        .map(|v| v.trim_matches(['"', '\'', ' ', ';']).to_string())
        .unwrap_or_default();
    let fname = mods::safe_zip_name(&raw_name, &m.modidstr, &m.modversion);

    let tmp = stage_dir.join(format!(".{}.part", m.fileid));
    let bytes_total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let mut received: u64 = 0;
    let mut last: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("{}: download read failed: {e}", m.name))?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        hasher.update(&chunk);
        received += chunk.len() as u64;
        if received - last >= 65_536 || received == bytes_total {
            last = received;
            emit_progress(app, pack_id, "download", &m.name, done, total, received, bytes_total);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let got = format!("{:x}", hasher.finalize());
    if got != m.sha256.to_ascii_lowercase() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{}: downloaded file does not match the pack's pin (expected sha256 {}, got {}). \
             The pack may need republishing; nothing was installed.",
            m.name, m.sha256, got
        ));
    }
    if !mods::looks_like_zip(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: downloaded file is not a valid mod zip", m.name));
    }

    let final_path = stage_dir.join(&fname);
    std::fs::rename(&tmp, &final_path).map_err(|e| format!("{}: could not stage: {e}", m.name))?;
    Ok(fname)
}

/// Stage-download and verify EVERY selected mod. Only returns Ok when the whole
/// set is on disk and verified; on any failure the stage dir is removed whole.
async fn stage_all(
    app: &AppHandle,
    stage_dir: &Path,
    selected: &[&ManifestMod],
    pack_id: &str,
) -> Result<Vec<String>, String> {
    let _ = std::fs::remove_dir_all(stage_dir);
    std::fs::create_dir_all(stage_dir).map_err(|e| format!("could not create stage dir: {e}"))?;
    let client = reqwest::Client::new();
    let mut files = Vec::with_capacity(selected.len());
    for (i, m) in selected.iter().enumerate() {
        match stage_one(app, &client, stage_dir, m, pack_id, i, selected.len()).await {
            Ok(f) => files.push(f),
            Err(e) => {
                let _ = std::fs::remove_dir_all(stage_dir);
                return Err(e);
            }
        }
    }
    Ok(files)
}

/// Make sure `game_version` is in the shared version cache, downloading it via
/// the official manifest if needed, and kick off the background Optimum build
/// for it (packs go through the same door as manual installs).
async fn ensure_game_version(app: &AppHandle, game_version: &str) -> Result<(), String> {
    if versions::is_cached(app, game_version) {
        return Ok(());
    }
    let available = versions::fetch_available(app).await?;
    let v = available
        .iter()
        .find(|a| a.version == game_version)
        .ok_or_else(|| {
            format!(
                "This pack targets Vintage Story {game_version}, which the official version \
                 manifest does not offer. Install that version manually first."
            )
        })?;
    versions::ensure_version(app, game_version, &v.url, &v.md5).await?;
    optimum::maybe_autobuild(app, game_version);
    Ok(())
}

/// Write the manifest's overrides on FIRST install: every override is written
/// and its canonical hash recorded (updates get the never-clobber rules).
fn apply_overrides_first_install(
    install_dir: &Path,
    manifest: &Manifest,
) -> Result<BTreeMap<String, String>, String> {
    use base64::Engine;
    let mut hashes = BTreeMap::new();
    for ov in &manifest.overrides {
        let dest = safe_override_path(install_dir, &ov.path)?;
        let bytes: Vec<u8> = match ov.encoding.as_str() {
            "base64" => base64::engine::general_purpose::STANDARD
                .decode(&ov.content)
                .map_err(|e| format!("override '{}': bad base64: {e}", ov.path))?,
            _ => ov.content.clone().into_bytes(),
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, &bytes).map_err(|e| format!("override '{}': {e}", ov.path))?;
        hashes.insert(ov.path.clone(), override_canonical_hash(&ov.path, &bytes));
    }
    Ok(hashes)
}

/// The whole install algorithm for a FRESH pack-managed installation
/// (spec steps 0-7). Returns the new install folder's path.
pub async fn install_pack(
    app: &AppHandle,
    installations_dir: &Path,
    hub_url: &str,
    pack_id: &str,
    install_name: &str,
    seed_from: Option<String>,
    optional_choices: BTreeMap<String, bool>,
) -> Result<String, String> {
    // Step 0: the id itself, before it is used to build any path. Checked here
    // rather than deeper down because `pack_id` reaches the filesystem in
    // step 3, and a rejected install is a far better outcome than a deleted
    // directory outside the installations folder.
    if !safe_pack_id(pack_id) {
        return Err(format!(
            "refusing to install '{pack_id}': a pack id must be 2 to 64 characters of lowercase letters, digits and hyphens"
        ));
    }

    // The manifest, and the launcher-version gate before anything else.
    let manifest = crate::hub::pack_manifest(hub_url, pack_id, None).await?;

    // The manifest's own id must agree with the one we asked for, or the
    // staging path and the freeze key would describe different packs.
    if manifest.pack.id != pack_id {
        return Err(format!(
            "the Hub returned a manifest for '{}' when asked for '{pack_id}'",
            manifest.pack.id
        ));
    }
    let app_version = env!("CARGO_PKG_VERSION");
    if !version_at_least(app_version, &manifest.pack.min_launcher_version) {
        return Err(format!(
            "This pack needs Translocator {} or newer (you have {}). Update the launcher first.",
            manifest.pack.min_launcher_version, app_version
        ));
    }

    // Fail the name collision NOW, not after a couple hundred downloads
    // (installations::create re-checks authoritatively at place time).
    if installations_dir.join(installations::slugify(install_name)).exists() {
        return Err(format!("a folder for '{install_name}' already exists"));
    }

    // Step 1: the game version, ensured BEFORE any download work.
    emit_progress(app, pack_id, "game", &manifest.pack.game_version, 0, 0, 0, 0);
    ensure_game_version(app, &manifest.pack.game_version).await?;

    // Step 2: selection.
    let selected = select_mods(&manifest, &optional_choices);

    // Step 3: stage + verify ALL before anything is placed.
    let stage_dir = installations_dir.join(format!(".pack-stage-{pack_id}"));
    let staged_files = stage_all(app, &stage_dir, &selected, pack_id).await?;

    // Step 4: only now create the install and place the verified set. Any
    // failure past this point removes the fresh folder whole, so a broken
    // install can never be left behind looking real.
    emit_progress(app, pack_id, "place", "", selected.len(), selected.len(), 0, 0);
    let install_path = installations::create(installations_dir, install_name, &manifest.pack.game_version)
        .map_err(|e| {
            let _ = std::fs::remove_dir_all(&stage_dir);
            e
        })?;
    let install_dir = PathBuf::from(&install_path);

    let placed = (|| -> Result<(), String> {
        if let Some(src) = &seed_from {
            let source = if src == "__base__" {
                tauri::Manager::path(app)
                    .data_dir()
                    .map_err(|e| e.to_string())?
                    .join("VintagestoryData")
            } else {
                PathBuf::from(src)
            };
            let _ = installations::seed_clientsettings(&source, &install_dir);
        }
        let mods_dir = install_dir.join("Mods");
        std::fs::create_dir_all(&mods_dir).map_err(|e| e.to_string())?;
        for f in &staged_files {
            std::fs::rename(stage_dir.join(f), mods_dir.join(f))
                .map_err(|e| format!("could not place {f}: {e}"))?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage_dir);
    if let Err(e) = placed {
        let _ = std::fs::remove_dir_all(&install_dir);
        return Err(e);
    }

    // Step 5: overrides (first install writes all).
    let override_hashes = match apply_overrides_first_install(&install_dir, &manifest) {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&install_dir);
            return Err(e);
        }
    };

    // Step 6: the pack's server, into the in-game multiplayer list.
    if let Some(server) = manifest.server.as_ref().filter(|s| s.auto_add) {
        let _ = session::add_multiplayer_server(&install_dir, &manifest.pack.name, &server.address, None);
    }

    // Step 7: freeze.
    let mut meta = installations::read_meta(&install_dir).unwrap_or_default();
    meta.name = install_name.to_string();
    meta.version = manifest.pack.game_version.clone();
    meta.managed_by = managed_key(&manifest.pack.id, &manifest.pack.version);
    meta.pack_strict = manifest.pack.strict;
    meta.pack_owned_files = staged_files;
    meta.override_hashes = override_hashes;
    meta.optional_choices = optional_choices;
    installations::write_meta(&install_dir, &meta)?;

    emit_progress(app, pack_id, "done", "", selected.len(), selected.len(), 0, 0);
    Ok(install_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_ids_that_reach_the_filesystem_are_narrow() {
        // The staging directory is `.pack-stage-{id}` and stage_all opens by
        // calling remove_dir_all on it, so anything that could climb out of the
        // installations folder has to be refused before it is joined to a path.
        let long_ok = "x".repeat(64);
        let too_long = "x".repeat(65);
        for good in ["the-quire-server-pack", "ok-2", "a1", long_ok.as_str()] {
            assert!(safe_pack_id(good), "should accept {good}");
        }
        let nul = String::from("bad") + &String::from_utf8(vec![0]).unwrap();
        for bad in [
            "../../../evil",
            "a/b",
            "a",
            "",
            "A-Pack",
            "-leading",
            "with space",
            "under_score",
            nul.as_str(),
            too_long.as_str(),
        ] {
            assert!(!safe_pack_id(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn the_naive_stage_path_really_did_escape() {
        // Documents the bug this guards, so nobody later decides the check is
        // paranoid: joined without validation, a traversing id climbs out.
        let hostile = "../../../../Windows";
        assert!(!safe_pack_id(hostile), "the guard must reject it");
        let joined = std::path::Path::new("C:/installs").join(format!(".pack-stage-{hostile}"));
        let climbs = joined
            .components()
            .filter(|c| matches!(c, std::path::Component::ParentDir))
            .count();
        assert!(climbs > 0, "the unvalidated join contains parent refs");
    }

    #[test]
    fn version_gate() {
        assert!(version_at_least("0.2.0", "0.2.0"));
        assert!(version_at_least("0.10.1", "0.2.0"));
        assert!(!version_at_least("0.1.9", "0.2.0"));
        assert!(version_at_least("1.0.0", "0.99.99"));
    }

    #[test]
    fn managed_roundtrip() {
        let k = managed_key("the-quire-server-pack", "0.1.0");
        assert_eq!(parse_managed(&k), Some(("the-quire-server-pack", "0.1.0")));
        assert_eq!(parse_managed("no-at-sign"), None);
    }

    #[test]
    fn override_hash_ignores_json_key_order() {
        let a = br#"{"b":1,"a":{"y":2,"x":3}}"#;
        let b = br#"{ "a": { "x": 3, "y": 2 }, "b": 1 }"#;
        assert_eq!(
            override_canonical_hash("ModConfig/x.json", a),
            override_canonical_hash("ModConfig/x.json", b)
        );
        assert_ne!(override_canonical_hash("some.txt", a), override_canonical_hash("some.txt", b));
    }

    #[test]
    fn override_path_containment() {
        let root = Path::new("C:/installs/pack");
        assert!(safe_override_path(root, "ModConfig/a.json").is_ok());
        assert!(safe_override_path(root, "../evil.json").is_err());
        assert!(safe_override_path(root, "C:/evil.json").is_err());
        assert!(safe_override_path(root, "/evil.json").is_err());
        assert!(safe_override_path(root, "\\evil.json").is_err());
    }
}
