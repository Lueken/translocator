//! Update detection: for each installed mod, which ModDB releases are newer and
//! how compatible are they with the installation's game version.
//!
//! Improves on VS Launcher two ways: (1) it carries each release's `changelog`
//! so the UI can show the notes for every version between installed and latest
//! (VS Launcher shows none), and (2) fetches run concurrently so a 191-mod pack
//! doesn't crawl through 191 serial calls. Compatibility is a real major.minor
//! match (theirs was a raw `startsWith` that can false-match "1.190").

use crate::{deps, mods};
use futures::stream::StreamExt;
use serde::Serialize;
use std::cmp::Ordering;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use tauri::Emitter;

// How many mod-detail fetches to run at once.
const CONCURRENCY: usize = 16;

#[derive(Serialize, Clone)]
pub struct ReleaseInfo {
    pub modversion: String,
    /// "exact" (author-tagged this version), "minor" (same major.minor), or
    /// "unlikely" (no matching tag).
    pub compat: String,
    pub changelog: String,
    pub created: String,
}

#[derive(Serialize)]
pub struct ModUpdate {
    pub modid: String,
    pub name: String,
    pub assetid: u64,
    pub installed_version: String,
    pub installed_filename: String,
    /// Releases newer than installed, newest first.
    pub newer: Vec<ReleaseInfo>,
    /// Newest release that is at least major.minor compatible, if any.
    pub latest_compatible: Option<String>,
}

fn major_minor(v: &str) -> (u64, u64) {
    let v = v.trim_start_matches(|c| c == 'v' || c == 'V');
    let mut it = v.split(|c| c == '.' || c == '-');
    let maj = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let min = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (maj, min)
}

fn numeric_cmp(a: &str, b: &str) -> Ordering {
    let parts = |s: &str| {
        s.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|x| x.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

/// Is `candidate` a newer version than `installed`? Prefers real semver; falls
/// back to numeric compare for non-semver-shaped VS versions.
fn is_newer(installed: &str, candidate: &str) -> bool {
    let norm = |s: &str| s.trim_start_matches(|c| c == 'v' || c == 'V').to_string();
    if let (Ok(a), Ok(b)) = (
        semver::Version::parse(&norm(installed)),
        semver::Version::parse(&norm(candidate)),
    ) {
        return b > a;
    }
    numeric_cmp(&norm(candidate), &norm(installed)) == Ordering::Greater
}

/// Compatibility of a release's game-version `tags` with the install version.
fn compat_of(tags: &[String], game_version: &str) -> String {
    let gv = game_version.trim_start_matches(|c| c == 'v' || c == 'V');
    let (gm, gn) = major_minor(game_version);
    let mut minor = false;
    for t in tags {
        let tt = t.trim_start_matches(|c| c == 'v' || c == 'V');
        if tt == gv {
            return "exact".into();
        }
        let (m, n) = major_minor(t);
        if m == gm && n == gn {
            minor = true;
        }
    }
    if minor {
        "minor".into()
    } else {
        "unlikely".into()
    }
}

/// For every installed mod, find newer releases + their compatibility and
/// changelog. Only mods with at least one newer release are returned.
/// Emits "check-progress" { done, total } as each mod resolves.
pub async fn check_updates(
    app: &tauri::AppHandle,
    install_dir: &Path,
    game_version: &str,
) -> Result<Vec<ModUpdate>, String> {
    let installed = deps::installed_mods(install_dir);
    let client = reqwest::Client::new();
    let game_version = game_version.to_string();
    let total = installed.len();
    let done = Arc::new(AtomicUsize::new(0));

    let futs = installed.into_iter().map(|(filename, info)| {
        let client = client.clone();
        let game_version = game_version.clone();
        let app = app.clone();
        let done = done.clone();
        async move {
            let full = mods::fetch_full(&client, &info.modid).await;
            let d = done.fetch_add(1, AtomicOrdering::Relaxed) + 1;
            let _ = app.emit("check-progress", serde_json::json!({ "done": d, "total": total }));
            let full = full.ok()?;
            let newer: Vec<ReleaseInfo> = full
                .releases
                .iter()
                .filter(|r| !r.modversion.is_empty() && is_newer(&info.version, &r.modversion))
                .map(|r| ReleaseInfo {
                    modversion: r.modversion.clone(),
                    compat: compat_of(&r.tags, &game_version),
                    changelog: r.changelog.clone(),
                    created: r.created.clone(),
                })
                .collect();
            if newer.is_empty() {
                return None;
            }
            let latest_compatible = newer
                .iter()
                .find(|r| r.compat != "unlikely")
                .map(|r| r.modversion.clone());
            let name = if full.name.is_empty() { info.name.clone() } else { full.name };
            Some(ModUpdate {
                modid: info.modid,
                name,
                assetid: full.assetid,
                installed_version: info.version,
                installed_filename: filename,
                newer,
                latest_compatible,
            })
        }
    });

    let mut out: Vec<ModUpdate> = futures::stream::iter(futs)
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}
