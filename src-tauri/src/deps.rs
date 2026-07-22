//! Self-reliant dependency checking from each mod's `modinfo.json`.
//!
//! Reads the `dependencies` map out of a mod zip and reports which required
//! mods are not present in the target installation. With the UI looping over
//! results this gives Translocator real transitive dependency resolution today,
//! without waiting on ModDB's relations API (anegostudios/vsmoddb#117). We can
//! swap to `install-information?resolve-deps=1` once that lands.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

// Base-game "mods" that ship with VS and are never on ModDB.
const BASE_MODS: &[&str] = &["game", "survival", "creative"];

/// Parse a modinfo.json blob (JSON5, case-insensitive keys) into
/// (modid, dependencies). Modids are lowercased for stable comparison.
fn parse_modinfo(text: &str) -> Option<(String, HashMap<String, String>)> {
    let v: serde_json::Value = json5::from_str(text).ok()?;
    let obj = v.as_object()?;
    let get = |key: &str| {
        obj.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, val)| val)
    };
    let modid = get("modid").and_then(|v| v.as_str()).unwrap_or_default().to_lowercase();
    let mut deps = HashMap::new();
    if let Some(d) = get("dependencies").and_then(|v| v.as_object()) {
        for (k, val) in d {
            deps.insert(k.to_lowercase(), val.as_str().unwrap_or("").to_string());
        }
    }
    Some((modid, deps))
}

/// Read (modid, dependencies) from a mod zip's root modinfo.json.
fn read_modinfo_from_zip(zip_path: &Path) -> Option<(String, HashMap<String, String>)> {
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

/// Lowercased modids of every mod already installed in `<install>/Mods`.
pub fn installed_modids(install_dir: &Path) -> HashSet<String> {
    let mods_dir = install_dir.join("Mods");
    let mut set = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(&mods_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                if let Some((modid, _)) = read_modinfo_from_zip(&p) {
                    if !modid.is_empty() {
                        set.insert(modid);
                    }
                }
            }
        }
    }
    set
}

#[derive(Serialize)]
pub struct MissingDep {
    pub modid: String,
    pub version: String,
}

/// Required dependencies of `mod_zip` that are not installed in `install_dir`
/// (skipping the base game). Modids are lowercased, which matches the ModDB
/// modidstr for installing them.
pub fn check_missing_deps(install_dir: &Path, mod_zip: &Path) -> Vec<MissingDep> {
    let installed = installed_modids(install_dir);
    let mut missing = Vec::new();
    if let Some((_, deps)) = read_modinfo_from_zip(mod_zip) {
        for (modid, version) in deps {
            if BASE_MODS.contains(&modid.as_str()) || installed.contains(&modid) {
                continue;
            }
            missing.push(MissingDep { modid, version });
        }
    }
    missing
}
