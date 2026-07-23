//! ModDB browsing, detail, and installing.
//!
//! Search + install hit the official ModDB API (`mods.vintagestory.at/api`).
//! Installing downloads a release's `mainfile` zip straight into the
//! installation's `Mods/` folder - no re-hosting, the license-safe model in the
//! brief. `fetch_full` is the shared detail fetch (releases with tags +
//! changelogs) that the update view and donation lookup both build on.

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use tauri::Emitter;

const MODDB: &str = "https://mods.vintagestory.at/api";

/// A mod download must be HTTPS and served from the official domain. The URL
/// comes from the ModDB API response, so we don't blindly fetch wherever it
/// points; this stops a compromised or spoofed API answer from redirecting a
/// download to an attacker's host.
fn is_allowed_mod_url(url: &str) -> bool {
    let rest = match url.strip_prefix("https://") {
        Some(r) => r,
        None => return false,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host); // drop any userinfo
    let host = host.split(':').next().unwrap_or(host); // drop any port
    host == "vintagestory.at" || host.ends_with(".vintagestory.at")
}

/// Reduce an API-supplied filename to a safe basename that can only ever land
/// inside `Mods/`: no path separators, no parent refs, `.zip` only. Anything
/// suspicious falls back to a name we construct ourselves.
fn safe_zip_name(raw: &str, modidstr: &str, modversion: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let looks_safe = !base.is_empty()
        && !base.contains("..")
        && base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')' | '+'))
        && base.to_ascii_lowercase().ends_with(".zip");
    if looks_safe {
        return base.to_string();
    }
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
            .collect()
    };
    format!("{}-{}.zip", clean(modidstr), clean(modversion))
}

/// The first bytes of every PKZip file. VS mods are zips; checking this stops a
/// wrong URL or an HTML error page from being saved into Mods/ as a `.zip`.
fn looks_like_zip(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .map(|_| &buf == b"PK")
        .unwrap_or(false)
}

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
    modid: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    assetid: u64,
    /// Which side loads the mod: "client" | "server" | "both" (VS may say
    /// "universal"). Used to build a pack's per-mod `side`.
    #[serde(default)]
    side: String,
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
    /// The download pin: `/download?fileid=<fileid>`.
    #[serde(default)]
    pub fileid: u64,
    /// The release record id (links to its changelog).
    #[serde(default)]
    pub releaseid: u64,
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
    pub modid: u64,
    pub name: String,
    pub assetid: u64,
    pub side: String,
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
        modid: d.modid,
        name: d.name,
        assetid: d.assetid,
        side: d.side,
        text: d.text,
        donateurl: d.donateurl,
        releases: d.releases,
    })
}

// ---------------------------------------------------------------- install

/// Stream a release into `<install>/Mods/`, emitting "install-progress" as bytes
/// arrive. Downloads to a temp `.part` file first, then swaps it into place and
/// removes the old zip only after a complete download, so a failed or
/// interrupted download never destroys the working copy.
async fn download_release(
    app: &tauri::AppHandle,
    client: &reqwest::Client,
    install_dir: &Path,
    modidstr: &str,
    rel: &FullRelease,
    old_filename: Option<&str>,
) -> Result<String, String> {
    let mods_dir = install_dir.join("Mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| e.to_string())?;

    if !is_allowed_mod_url(&rel.mainfile) {
        return Err(format!(
            "refused mod download from an unexpected URL ({}); expected an https vintagestory.at address",
            rel.mainfile
        ));
    }
    let fname = safe_zip_name(&rel.filename, modidstr, &rel.modversion);
    let tmp = mods_dir.join(format!(".{fname}.part"));

    let resp = client
        .get(&rel.mainfile)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    let total = resp.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut received: u64 = 0;
    let mut last: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("download read failed: {e}"));
            }
        };
        if let Err(e) = file.write_all(&chunk) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.to_string());
        }
        received += chunk.len() as u64;
        if received - last >= 32_768 || received == total {
            last = received;
            let _ = app.emit(
                "install-progress",
                serde_json::json!({ "modid": modidstr, "received": received, "total": total }),
            );
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    // Reject anything that isn't actually a zip (a redirect to an HTML error
    // page, a truncated body) before it ever lands in Mods/.
    if !looks_like_zip(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err("downloaded file is not a valid mod zip; discarded".into());
    }

    // Swap only after a complete download: remove the old zip, move temp in.
    if let Some(old) = old_filename {
        let p = mods_dir.join(old);
        if p.exists() {
            let _ = std::fs::remove_file(p);
        }
    }
    let final_path = mods_dir.join(&fname);
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }
    std::fs::rename(&tmp, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not place downloaded mod: {e}")
    })?;
    Ok(fname)
}

/// Install a mod's latest release (used by dependency resolution).
pub async fn install_latest(
    app: &tauri::AppHandle,
    install_dir: &Path,
    modidstr: &str,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let full = fetch_full(&client, modidstr).await?;
    let rel = full.releases.first().ok_or("mod has no releases")?;
    download_release(app, &client, install_dir, modidstr, rel, None).await
}

/// Install a specific release version, replacing `old_filename` if given.
pub async fn install_release(
    app: &tauri::AppHandle,
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
    download_release(app, &client, install_dir, modidstr, &rel, old_filename).await
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
