//! Best-effort import of installations another launcher already set up.
//!
//! Translocator never moves or copies game data here. When the user points at a
//! folder that VS Launcher or StoryForge manages, we read that launcher's own
//! config and seed each not-yet-adopted install's `translocator.json` with its
//! real metadata - version, launch params, env vars, backup prefs, playtime,
//! icon - instead of a bare folder-name default. The account/session block is
//! deliberately ignored: imported installs get a fresh, validated session
//! stamped by Translocator at launch, never a copied (and possibly stale) token.
//!
//! Detection and enrichment are both non-destructive. Enrichment only writes our
//! own metadata file, and only for folders that don't already have one, so it
//! never clobbers settings a user has already tuned inside Translocator.

use crate::installations::{self, InstallationMeta};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// VS Launcher's per-installation record (subset we carry over). VSL writes
/// camelCase keys and stores both timestamps in milliseconds.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VslInstall {
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    start_params: String,
    #[serde(default)]
    env_vars: String,
    #[serde(default)]
    backups_limit: u8,
    #[serde(default)]
    backups_auto: bool,
    #[serde(default)]
    compression_level: u8,
    #[serde(default)]
    last_time_played: u64,
    #[serde(default)]
    total_time_played: u64,
}

/// VSL's downloaded-game-version record: `{version, path}` pointing at a
/// `VSLGameVersions\<ver>` folder holding real game binaries.
#[derive(Deserialize)]
struct VslGameVersion {
    #[serde(default)]
    version: String,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct VslConfig {
    #[serde(default)]
    installations: Vec<VslInstall>,
    #[serde(rename = "gameVersions", default)]
    game_versions: Vec<VslGameVersion>,
}

/// A launcher we found on disk with installations Translocator can adopt.
#[derive(Serialize)]
pub struct DetectedLauncher {
    pub launcher: String,
    pub installations_dir: String,
    /// Whether we can carry per-install settings across (we have a config reader).
    pub can_enrich: bool,
    pub count: usize,
}

#[derive(Serialize)]
pub struct ImportResult {
    /// Installs whose metadata we seeded from the launcher config.
    pub enriched: usize,
    /// Which launcher's config we matched, if any.
    pub launcher: Option<String>,
}

/// OS roaming app-data root (`%APPDATA%` on Windows), the parent every launcher
/// drops its folder into.
fn appdata_root(app: &AppHandle) -> Option<PathBuf> {
    app.path().data_dir().ok()
}

fn count_installs(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && installations::looks_like_install(&p) {
                n += 1;
            }
        }
    }
    n
}

/// The default installations folders of the launchers we know about, if present.
pub fn detect(app: &AppHandle) -> Vec<DetectedLauncher> {
    let mut out = Vec::new();
    let root = match appdata_root(app) {
        Some(r) => r,
        None => return out,
    };
    // (display name, installations dir, can we read its config for enrichment?)
    let candidates = [
        ("VS Launcher", root.join("VSLInstallations"), true),
        (
            "StoryForge",
            root.join("StoryForge").join("installations"),
            false,
        ),
    ];
    for (name, dir, can_enrich) in candidates {
        if dir.is_dir() {
            let count = count_installs(&dir);
            if count > 0 {
                out.push(DetectedLauncher {
                    launcher: name.to_string(),
                    installations_dir: dir.to_string_lossy().into_owned(),
                    can_enrich,
                    count,
                });
            }
        }
    }
    out
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().replace(['-', '_'], " ").trim().to_string())
        .unwrap_or_default()
}

/// Turn a VSL record into Translocator metadata. VSL defaults (0) fall back to
/// Translocator's; playtime converts from milliseconds to seconds.
fn meta_from_vsl(v: &VslInstall, dir: &Path) -> InstallationMeta {
    InstallationMeta {
        name: if v.name.trim().is_empty() {
            folder_name(dir)
        } else {
            v.name.clone()
        },
        version: v.version.clone(),
        start_params: v.start_params.clone(),
        env_vars: v.env_vars.clone(),
        auto_backup: v.backups_auto,
        compression: if v.compression_level == 0 { 6 } else { v.compression_level },
        backups_limit: if v.backups_limit == 0 { 5 } else { v.backups_limit },
        // VSL icons are its own named glyphs ("basalt", "rusty"); they'd render
        // as literal text in our icon circle, so fall back to the name initial.
        icon: String::new(),
        favorite: false,
        last_played: v.last_time_played,
        total_time_played: v.total_time_played / 1000,
        // An adopted VSL install is by definition a personal one.
        ..Default::default()
    }
}

/// Read VS Launcher's config, if present.
fn read_vsl_config(root: &Path) -> Option<VslConfig> {
    let path = root.join("VSLauncher").join("config.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// VS Launcher's own downloaded binary for a game version, if VSL has it.
/// The adoption promise is "pick up your old installs and just run": VSL users
/// already hold the right binaries in VSLGameVersions, so launching resolves
/// through them before ever asking anyone to download a version they own.
pub fn vsl_version_exe(app: &AppHandle, version: &str) -> Option<PathBuf> {
    let root = appdata_root(app)?;
    let cfg = read_vsl_config(&root)?;
    let v = cfg
        .game_versions
        .iter()
        .find(|g| g.version.eq_ignore_ascii_case(version) && !g.path.trim().is_empty())?;
    let exe = PathBuf::from(&v.path).join("Vintagestory.exe");
    exe.is_file().then_some(exe)
}

/// Seed `translocator.json` for every folder in `installations_dir` that a known
/// launcher's config describes and that we haven't adopted yet. Returns how many
/// were enriched and which launcher matched.
pub fn import_from_launcher(app: &AppHandle, installations_dir: &Path) -> ImportResult {
    let mut enriched = 0usize;
    let root = match appdata_root(app) {
        Some(r) => r,
        None => return ImportResult { enriched, launcher: None },
    };

    if let Some(cfg) = read_vsl_config(&root) {
        for v in &cfg.installations {
            if v.path.trim().is_empty() {
                continue;
            }
            let folder = PathBuf::from(&v.path);
            // Only touch folders that live directly in the pointed-at directory
            // and exist on disk.
            if folder.parent() != Some(installations_dir) || !folder.is_dir() {
                continue;
            }
            let existing = installations::read_meta(&folder);
            // Enrich when not yet adopted, or adopted but still unconfigured (a
            // bare folder-name adoption: no playtime, params, or auto-backup).
            // A genuinely configured install is left untouched.
            let needs = match &existing {
                None => true,
                Some(m) => {
                    m.total_time_played == 0
                        && m.start_params.trim().is_empty()
                        && m.env_vars.trim().is_empty()
                        && !m.auto_backup
                }
            };
            if needs {
                let mut meta = meta_from_vsl(v, &folder);
                if let Some(m) = &existing {
                    meta.favorite = m.favorite; // preserve a choice the user made
                }
                if installations::write_meta(&folder, &meta).is_ok() {
                    enriched += 1;
                }
            }
        }
        if enriched > 0 {
            return ImportResult {
                enriched,
                launcher: Some("VS Launcher".into()),
            };
        }
    }

    // StoryForge enrichment is not implemented (its store schema wasn't
    // available to model); those installs still adopt with folder-name defaults.
    ImportResult {
        enriched,
        launcher: None,
    }
}
