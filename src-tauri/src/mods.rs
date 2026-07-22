//! ModDB browsing + installing.
//!
//! Search hits the official ModDB API (`mods.vintagestory.at/api/mods`); install
//! resolves a mod's latest release and downloads its `mainfile` zip straight
//! into the installation's `Mods/` folder — no re-hosting, exactly the
//! license-safe model in the brief. Game-version compatibility filtering is a
//! later concern; this prototype searches by text and installs the newest
//! release.

use serde::{Deserialize, Serialize};
use std::path::Path;

const MODDB: &str = "https://mods.vintagestory.at/api";

#[derive(Deserialize)]
struct ModsListResponse {
    #[serde(default)]
    mods: Vec<ModSummaryRaw>,
}

#[derive(Deserialize)]
struct ModSummaryRaw {
    modid: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    modidstrs: Vec<String>,
}

#[derive(Serialize)]
pub struct ModSummary {
    pub modid: u32,
    pub modidstr: String,
    pub name: String,
    pub summary: String,
    pub author: String,
    pub downloads: u64,
}

/// Text search against ModDB, most-downloaded first (top 30).
pub async fn search(text: &str) -> Result<Vec<ModSummary>, String> {
    let resp = reqwest::Client::new()
        .get(format!("{MODDB}/mods"))
        .query(&[("text", text), ("orderby", "downloads")])
        .send()
        .await
        .map_err(|e| format!("mod search failed: {e}"))?;
    let data: ModsListResponse = resp.json().await.map_err(|e| format!("mod search parse: {e}"))?;
    Ok(data
        .mods
        .into_iter()
        .take(30)
        .map(|m| ModSummary {
            modidstr: m.modidstrs.first().cloned().unwrap_or_else(|| m.modid.to_string()),
            modid: m.modid,
            name: m.name,
            summary: m.summary.unwrap_or_default(),
            author: m.author.unwrap_or_default(),
            downloads: m.downloads,
        })
        .collect())
}

#[derive(Deserialize)]
struct ModDetailResponse {
    #[serde(rename = "mod")]
    mod_detail: ModDetail,
}

#[derive(Deserialize)]
struct ModDetail {
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Deserialize)]
struct Release {
    mainfile: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    modversion: String,
}

/// Resolve the mod's latest release and download it into `<install>/Mods/`.
/// Returns the installed filename.
pub async fn install_latest(install_dir: &Path, modidstr: &str) -> Result<String, String> {
    let detail: ModDetailResponse = reqwest::Client::new()
        .get(format!("{MODDB}/mod/{modidstr}"))
        .send()
        .await
        .map_err(|e| format!("mod detail failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("mod detail parse: {e}"))?;

    // ModDB returns releases newest-first.
    let rel = detail
        .mod_detail
        .releases
        .first()
        .ok_or("mod has no releases")?;

    let bytes = reqwest::Client::new()
        .get(&rel.mainfile)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download read failed: {e}"))?;

    let mods_dir = install_dir.join("Mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| e.to_string())?;
    let fname = if rel.filename.is_empty() {
        format!("{modidstr}-{}.zip", rel.modversion)
    } else {
        rel.filename.clone()
    };
    std::fs::write(mods_dir.join(&fname), &bytes).map_err(|e| e.to_string())?;
    Ok(fname)
}

/// Zip filenames currently in the installation's Mods folder.
pub fn list_mod_files(install_dir: &Path) -> Vec<String> {
    let mods_dir = install_dir.join("Mods");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&mods_dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.to_lowercase().ends_with(".zip") {
                out.push(n);
            }
        }
    }
    out.sort();
    out
}
