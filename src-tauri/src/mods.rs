//! ModDB browsing, detail, and installing.
//!
//! Search + install hit the official ModDB API (`mods.vintagestory.at/api`).
//! Installing downloads a release's `mainfile` zip straight into the
//! installation's `Mods/` folder — no re-hosting, the license-safe model in the
//! brief. `fetch_full` is the shared detail fetch (releases with tags +
//! changelogs) that the update view and donation lookup both build on.

use serde::{Deserialize, Serialize};
use std::path::Path;

const MODDB: &str = "https://mods.vintagestory.at/api";

// ---------------------------------------------------------------- search

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

// ---------------------------------------------------------------- detail

#[derive(Deserialize)]
struct FullModResponse {
    #[serde(rename = "mod")]
    mod_detail: FullModDetail,
}

#[derive(Deserialize)]
struct FullModDetail {
    #[serde(default)]
    name: String,
    #[serde(default)]
    assetid: u64,
    #[serde(default)]
    text: String,
    // Present once upstream exposes it (anegostudios/vsmoddb#143); harmless until then.
    #[serde(default)]
    donateurl: Option<String>,
    #[serde(default)]
    releases: Vec<FullRelease>,
}

/// A ModDB release. `tags` are the compatible game versions; `changelog` is the
/// per-release notes (which the existing launchers never surface).
#[derive(Deserialize, Clone)]
pub struct FullRelease {
    pub mainfile: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub modversion: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub changelog: String,
    #[serde(default)]
    pub created: String,
}

pub struct ModFull {
    pub name: String,
    pub assetid: u64,
    pub text: String,
    pub donateurl: Option<String>,
    pub releases: Vec<FullRelease>,
}

/// Fetch a mod's full detail (releases newest-first). Shared by install, the
/// update view, and donation lookup.
pub async fn fetch_full(client: &reqwest::Client, modidstr: &str) -> Result<ModFull, String> {
    let r: FullModResponse = client
        .get(format!("{MODDB}/mod/{modidstr}"))
        .send()
        .await
        .map_err(|e| format!("mod detail failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("mod detail parse ({modidstr}): {e}"))?;
    let d = r.mod_detail;
    Ok(ModFull {
        name: d.name,
        assetid: d.assetid,
        text: d.text,
        donateurl: d.donateurl,
        releases: d.releases,
    })
}

// ---------------------------------------------------------------- install

async fn download_release(
    client: &reqwest::Client,
    install_dir: &Path,
    modidstr: &str,
    rel: &FullRelease,
    old_filename: Option<&str>,
) -> Result<String, String> {
    let mods_dir = install_dir.join("Mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| e.to_string())?;
    if let Some(old) = old_filename {
        let p = mods_dir.join(old);
        if p.exists() {
            let _ = std::fs::remove_file(p);
        }
    }
    let bytes = client
        .get(&rel.mainfile)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download read failed: {e}"))?;
    let fname = if rel.filename.is_empty() {
        format!("{modidstr}-{}.zip", rel.modversion)
    } else {
        rel.filename.clone()
    };
    std::fs::write(mods_dir.join(&fname), &bytes).map_err(|e| e.to_string())?;
    Ok(fname)
}

/// Install a mod's latest release (used by dependency resolution).
pub async fn install_latest(install_dir: &Path, modidstr: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let full = fetch_full(&client, modidstr).await?;
    let rel = full.releases.first().ok_or("mod has no releases")?;
    download_release(&client, install_dir, modidstr, rel, None).await
}

/// Install a specific release version, replacing `old_filename` if given.
pub async fn install_release(
    install_dir: &Path,
    modidstr: &str,
    modversion: &str,
    old_filename: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let full = fetch_full(&client, modidstr).await?;
    let rel = full
        .releases
        .iter()
        .find(|r| r.modversion == modversion)
        .ok_or_else(|| format!("release {modversion} not found for {modidstr}"))?
        .clone();
    download_release(&client, install_dir, modidstr, &rel, old_filename).await
}

// ---------------------------------------------------------------- donations

/// Donation links for a mod: the API's `donateurl` field if present (once
/// upstream exposes it), otherwise donation URLs parsed from the description.
pub async fn get_donations(modidstr: &str) -> Result<Vec<String>, String> {
    let full = fetch_full(&reqwest::Client::new(), modidstr).await?;
    if let Some(url) = full.donateurl.filter(|s| !s.trim().is_empty()) {
        return Ok(vec![url]);
    }
    Ok(extract_donation_links(&full.text))
}

/// Extract donation URLs (Patreon, Ko-fi, PayPal, etc.) from an HTML blob.
fn extract_donation_links(html: &str) -> Vec<String> {
    const HOSTS: &[&str] = &[
        "patreon.com",
        "ko-fi.com",
        "paypal.me",
        "paypal.com",
        "buymeacoffee.com",
        "liberapay.com",
        "github.com/sponsors",
        "boosty.to",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("http") {
        let start = &rest[pos..];
        let end = start
            .find(|c: char| {
                matches!(c, '"' | '\'' | '<' | '>' | ')' | ' ' | '\n' | '\r' | '\t' | '\\')
            })
            .unwrap_or(start.len());
        let url = &start[..end];
        if HOSTS.iter().any(|h| url.contains(h)) && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
        rest = &start[end.max(4)..];
    }
    out
}

// ---------------------------------------------------------------- installed files

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
