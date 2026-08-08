//! Curator: build a modpack manifest from an installation, then publish it.
//!
//! For each mod zip in `Mods/`, read its modid+version from `modinfo.json`,
//! resolve it on ModDB to the exact release (fileid, releaseid, side), and pin it
//! with a locally-computed SHA-256. ModDB-only: a mod that isn't on ModDB, or
//! whose installed version isn't a published release, can't be pinned and is
//! reported so the publisher knows exactly what to post.

use crate::deps;
use crate::manifest::*;
use crate::mods;
use base64::Engine;
use futures::stream::{self, StreamExt};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

/// A mod we could not pin to a ModDB release (the publisher's to-do list).
#[derive(Serialize, Clone)]
pub struct Unresolved {
    pub filename: String,
    pub modid: String,
    pub version: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct CuratorPreview {
    pub manifest: Manifest,
    pub unresolved: Vec<Unresolved>,
    pub resolved_count: usize,
}

fn sha256_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

/// ModDB `side` -> manifest side. VS/ModDB uses "universal" for both.
fn normalize_side(s: &str) -> String {
    match s.trim().to_lowercase().as_str() {
        "server" => "server",
        "client" => "client",
        _ => "both",
    }
    .to_string()
}

enum Outcome {
    Resolved(Box<ManifestMod>),
    Unresolved(Unresolved),
}

async fn resolve_one(
    client: &reqwest::Client,
    mods_dir: &Path,
    filename: String,
    info: deps::ModInfo,
) -> Outcome {
    // A real ModDB 404 means "not posted there". Anything else (rate limit,
    // network hiccup, HTML error page) is retried with backoff and, if it still
    // fails, reported as a lookup failure - NOT as "not on ModDB".
    let mut attempt: u32 = 0;
    let full = loop {
        match mods::fetch_full_checked(client, &info.modid).await {
            Ok(f) => break f,
            Err(mods::FetchErr::NotFound) => {
                return Outcome::Unresolved(Unresolved {
                    filename,
                    modid: info.modid,
                    version: info.version,
                    reason: "not found on ModDB (post it there first)".into(),
                });
            }
            Err(mods::FetchErr::Other(e)) => {
                if attempt >= 3 {
                    return Outcome::Unresolved(Unresolved {
                        filename,
                        modid: info.modid,
                        version: info.version,
                        reason: format!("ModDB lookup failed after retries ({e}); rebuild to try again"),
                    });
                }
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt))).await;
            }
        }
    };
    // Version match must also carry a real file: ModDB has file-less release
    // records (fileid null -> 0), which can't be pinned or downloaded.
    let Some(rel) = full
        .releases
        .iter()
        .find(|r| r.modversion.eq_ignore_ascii_case(&info.version) && r.fileid > 0)
    else {
        let seen: Vec<&str> = full.releases.iter().map(|r| r.modversion.as_str()).take(4).collect();
        return Outcome::Unresolved(Unresolved {
            filename,
            modid: info.modid,
            version: info.version.clone(),
            reason: format!("version {} not on ModDB (has: {})", info.version, seen.join(", ")),
        });
    };
    let zip_path = mods_dir.join(&filename);
    let sha = tokio::task::spawn_blocking(move || sha256_file(&zip_path)).await.ok().flatten();
    let Some(sha256) = sha else {
        return Outcome::Unresolved(Unresolved {
            filename,
            modid: info.modid,
            version: info.version,
            reason: "could not read/hash the mod file".into(),
        });
    };
    Outcome::Resolved(Box::new(ManifestMod {
        modid: full.modid,
        modidstr: info.modid.clone(),
        name: if info.name.is_empty() { full.name } else { info.name },
        modversion: info.version,
        fileid: rel.fileid,
        releaseid: if rel.releaseid > 0 { Some(rel.releaseid) } else { None },
        side: normalize_side(&full.side),
        sha256,
        required: true,
    }))
}

/// A relative path inside the install is safe if it never escapes the dataPath.
fn safe_rel(rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    if norm.is_empty() || norm.starts_with('/') || norm.contains(':') {
        return false;
    }
    !norm.split('/').any(|p| p == ".." || p == ".")
}

/// Read an override file from the install, inlining it (utf8, or base64 if binary).
fn read_override(install_dir: &Path, rel: &str) -> Result<ManifestOverride, String> {
    if !safe_rel(rel) {
        return Err(format!("unsafe override path: {rel}"));
    }
    let bytes = std::fs::read(install_dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    match String::from_utf8(bytes.clone()) {
        Ok(text) => Ok(ManifestOverride { path: rel.replace('\\', "/"), encoding: "utf8".into(), content: text }),
        Err(_) => Ok(ManifestOverride {
            path: rel.replace('\\', "/"),
            encoding: "base64".into(),
            content: base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    }
}

/// Build a manifest preview from an installation: resolved mods + the unresolved
/// report. Publishing is a separate, explicit step.
pub async fn build_manifest(
    install_dir: &Path,
    pack: ManifestPack,
    links: Option<ManifestLinks>,
    server: Option<ManifestServer>,
    override_paths: Vec<String>,
) -> Result<CuratorPreview, String> {
    let client = reqwest::Client::new();
    let mods_dir = install_dir.join("Mods");

    let outcomes: Vec<Outcome> = stream::iter(deps::installed_mods(install_dir).into_iter().map(|(filename, info)| {
        let client = client.clone();
        let mods_dir = mods_dir.clone();
        async move { resolve_one(&client, &mods_dir, filename, info).await }
    }))
    // Modest concurrency: 200 near-simultaneous lookups trip ModDB's rate
    // limiting, which is what the retry above then has to absorb.
    .buffer_unordered(5)
    .collect()
    .await;

    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for o in outcomes {
        match o {
            Outcome::Resolved(m) => resolved.push(*m),
            Outcome::Unresolved(u) => unresolved.push(u),
        }
    }
    resolved.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    unresolved.sort_by(|a, b| a.filename.to_lowercase().cmp(&b.filename.to_lowercase()));

    let mut overrides = Vec::new();
    for rel in override_paths {
        overrides.push(read_override(install_dir, &rel)?);
    }

    let resolved_count = resolved.len();
    Ok(CuratorPreview {
        manifest: Manifest {
            manifest_version: MANIFEST_VERSION,
            pack,
            links,
            server,
            mods: resolved,
            overrides,
        },
        unresolved,
        resolved_count,
    })
}

/// Publish a signed manifest to the Hub. The envelope is
/// `{ manifest, signature, uid }`: the Hub rebuilds the canonical payload
/// (which binds the uid) and verifies the signature against one of that VS
/// account's registered device keys. The signature IS the auth; there is no
/// token. `manifest_value` must be the exact JSON value the payload was
/// derived from, so Hub and launcher canonicalize identical bytes.
pub async fn publish(
    hub_url: &str,
    pack_id: &str,
    manifest_value: &serde_json::Value,
    signature: &str,
    uid: &str,
) -> Result<String, String> {
    let base = hub_url.trim_end_matches('/');
    let url = format!("{base}/api/packs/{pack_id}/versions");
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({
            "manifest": manifest_value,
            "signature": signature,
            "uid": uid,
        }))
        .send()
        .await
        .map_err(|e| format!("publish request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("publish rejected ({status}): {body}"))
    }
}

/// List `ModConfig/*.json` files in an install, as install-relative paths, so the
/// UI can offer them as override candidates.
pub fn list_config_files(install_dir: &Path) -> Vec<String> {
    let dir = install_dir.join("ModConfig");
    let mut out = Vec::new();
    collect_json(&dir, install_dir, &mut out);
    out.sort();
    out
}

fn collect_json(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_json(&p, root, out);
        } else if p.extension().map(|x| x.eq_ignore_ascii_case("json")).unwrap_or(false) {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}
