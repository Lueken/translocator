//! Reading `modinfo.json` out of installed mod zips: identity, version, and
//! declared dependencies. Powers both the dependency check and the update view.
//!
//! Self-reliant (no ModDB call needed to know what's installed). Dependency
//! resolution can swap to ModDB's relations API later (anegostudios/vsmoddb#117).

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

// Base-game "mods" that ship with VS and are never on ModDB.
const BASE_MODS: &[&str] = &["game", "survival", "creative"];

#[derive(Clone)]
pub struct ModInfo {
    pub modid: String,
    pub version: String,
    pub name: String,
    /// modinfo.json `side`, lowercased ("universal" / "client" / "server");
    /// empty when the field is absent. This is what the game itself enforces.
    pub side: String,
    pub dependencies: HashMap<String, String>,
}

/// Parse a modinfo.json blob (JSON5, case-insensitive keys). Modid is lowercased
/// for stable comparison; version/name are kept as-authored.
fn parse_modinfo(text: &str) -> Option<ModInfo> {
    let v: serde_json::Value = json5::from_str(text).ok()?;
    let obj = v.as_object()?;
    let get = |key: &str| {
        obj.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, val)| val)
    };
    let modid = get("modid").and_then(|v| v.as_str()).unwrap_or_default().to_lowercase();
    let version = get("version").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let name = get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let side = get("side").and_then(|v| v.as_str()).unwrap_or_default().to_lowercase();
    let mut dependencies = HashMap::new();
    if let Some(d) = get("dependencies").and_then(|v| v.as_object()) {
        for (k, val) in d {
            dependencies.insert(k.to_lowercase(), val.as_str().unwrap_or("").to_string());
        }
    }
    Some(ModInfo { modid, version, name, side, dependencies })
}

/// Read the ModInfo from a mod zip's root modinfo.json.
fn read_modinfo_from_zip(zip_path: &Path) -> Option<ModInfo> {
    let file = std::fs::File::open(zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut idx = None;
    for i in 0..archive.len() {
        if let Ok(f) = archive.by_index(i) {
            if f.name().eq_ignore_ascii_case("modinfo.json") {
                idx = Some(i);
                break;
            }
        }
    }
    let mut f = archive.by_index(idx?).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    parse_modinfo(&s)
}

/// Every installed mod in `<install>/Mods`, paired with its zip filename.
pub fn installed_mods(install_dir: &Path) -> Vec<(String, ModInfo)> {
    let mods_dir = install_dir.join("Mods");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&mods_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                if let Some(info) = read_modinfo_from_zip(&p) {
                    if !info.modid.is_empty() {
                        let fname = e.file_name().to_string_lossy().into_owned();
                        out.push((fname, info));
                    }
                }
            }
        }
    }
    out
}

/// Lowercased modids of every mod already installed.
pub fn installed_modids(install_dir: &Path) -> HashSet<String> {
    installed_mods(install_dir)
        .into_iter()
        .map(|(_, info)| info.modid)
        .collect()
}

#[derive(Serialize)]
pub struct MissingDep {
    pub modid: String,
    pub version: String,
}

/// Required dependencies of `mod_zip` that are not installed in `install_dir`
/// (skipping the base game). Modids are lowercased, matching the ModDB modidstr.
pub fn check_missing_deps(install_dir: &Path, mod_zip: &Path) -> Vec<MissingDep> {
    let installed = installed_modids(install_dir);
    let mut missing = Vec::new();
    if let Some(info) = read_modinfo_from_zip(mod_zip) {
        for (modid, version) in info.dependencies {
            if BASE_MODS.contains(&modid.as_str()) || installed.contains(&modid) {
                continue;
            }
            missing.push(MissingDep { modid, version });
        }
    }
    missing
}
