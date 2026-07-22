//! Game version management with a shared, deduplicated binary cache.
//!
//! A VS version's Windows client is a ~570 MB Inno-Setup installer. Downloading
//! that per installation would be wasteful, so versions live ONCE in
//! `<app_data>/gameversions/<version>/` and every installation pinned to that
//! version launches against the same binary. The UI never needs a separate
//! "Versions" page: an installation asks for a version and we ensure it is
//! cached, downloading + installing only if it isn't already there.
//!
//! Windows-first: the client ships only as an Inno installer (no portable zip),
//! so `ensure_version` runs it silently into the cache folder — the same
//! mechanism VS Launcher uses.

use futures::StreamExt;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

const MANIFEST_URL: &str = "https://api.vintagestory.at/stable-unstable.json";

#[derive(Serialize, Clone)]
pub struct AvailableVersion {
    pub version: String,
    /// Preferred (cdn) download URL for the Windows installer.
    pub url: String,
    pub md5: String,
    /// Human-readable size from the manifest, e.g. "570.2 MB".
    pub filesize: String,
    pub cached: bool,
}

fn cache_root(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?
        .join("gameversions");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn version_dir(app: &AppHandle, version: &str) -> Result<PathBuf, String> {
    Ok(cache_root(app)?.join(version))
}

/// The cached game executable for a version, if it has been installed.
pub fn exe_path(app: &AppHandle, version: &str) -> Option<PathBuf> {
    let p = version_dir(app, version).ok()?.join("Vintagestory.exe");
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

pub fn is_cached(app: &AppHandle, version: &str) -> bool {
    exe_path(app, version).is_some()
}

/// Newest-first numeric compare of dotted versions ("1.22.5" > "1.22.4").
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
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
    std::cmp::Ordering::Equal
}

/// Fetch the official version manifest and return the installable Windows
/// versions, newest first, each flagged if already cached.
pub async fn fetch_available(app: &AppHandle) -> Result<Vec<AvailableVersion>, String> {
    let text = reqwest::Client::new()
        .get(MANIFEST_URL)
        .send()
        .await
        .map_err(|e| format!("version manifest failed: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("manifest parse: {e}"))?;

    let mut out = Vec::new();
    for (version, platforms) in map {
        let Some(win) = platforms.get("windows") else { continue };
        // urls is an object { cdn, local } — prefer cdn.
        let url = win
            .get("urls")
            .and_then(|u| u.get("cdn").or_else(|| u.get("local")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }
        out.push(AvailableVersion {
            cached: is_cached(app, &version),
            md5: win.get("md5").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            filesize: win.get("filesize").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            version,
            url,
        });
    }
    out.sort_by(|a, b| version_cmp(&b.version, &a.version));
    Ok(out)
}

/// Cached version strings (folders holding a Vintagestory.exe).
pub fn list_cached(app: &AppHandle) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(root) = cache_root(app) {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                if e.path().join("Vintagestory.exe").is_file() {
                    out.push(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    out.sort_by(|a, b| version_cmp(b, a));
    out
}

/// Remove a cached version's binaries. The caller checks nothing references it.
pub fn remove_cached(app: &AppHandle, version: &str) -> Result<(), String> {
    let dir = version_dir(app, version)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("remove failed: {e}"))?;
    }
    Ok(())
}

/// Ensure a version is installed in the cache and return its exe path. If it is
/// already cached this returns instantly (the dedup win). Otherwise it streams
/// the installer (emitting "version-progress") and runs it silently.
pub async fn ensure_version(app: &AppHandle, version: &str, url: &str) -> Result<String, String> {
    if let Some(p) = exe_path(app, version) {
        return Ok(p.to_string_lossy().into_owned());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (url, version);
        return Err("Automatic version install is Windows-only for now.".into());
    }

    #[cfg(target_os = "windows")]
    {
        let dir = version_dir(app, version)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let tmp = cache_root(app)?.join(format!("{version}-installer.exe"));

        // Stream the installer to a temp file with progress.
        let resp = reqwest::Client::new()
            .get(url)
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
            if received - last >= 1_048_576 || received == total {
                last = received;
                let _ = app.emit(
                    "version-progress",
                    serde_json::json!({ "version": version, "phase": "download", "received": received, "total": total }),
                );
            }
        }
        file.flush().map_err(|e| e.to_string())?;
        drop(file);

        // Run the Inno-Setup installer silently into the cache folder.
        let _ = app.emit(
            "version-progress",
            serde_json::json!({ "version": version, "phase": "install", "received": 0, "total": 0 }),
        );
        let status = tokio::process::Command::new(&tmp)
            .args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CURRENTUSER",
                "/NOICONS",
                &format!("/DIR={}", dir.display()),
            ])
            .status()
            .await
            .map_err(|e| format!("installer failed to run: {e}"))?;
        let _ = std::fs::remove_file(&tmp);
        if !status.success() {
            return Err(format!("installer exited with status {status}"));
        }

        match exe_path(app, version) {
            Some(p) => Ok(p.to_string_lossy().into_owned()),
            None => Err("installer finished but Vintagestory.exe was not found in the version folder".into()),
        }
    }
}
