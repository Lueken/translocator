//! Hub read API client: browse published packs and open a pack's page.
//!
//! The webview CSP forbids direct network calls, so every Hub request is routed
//! through these commands (reqwest, off the UI thread). Publishing lives in
//! `curator.rs`; this module is the read half the Market and Pack pages consume.

use crate::manifest::Manifest;
use serde::{Deserialize, Serialize};

/// One row in the Market list (mirrors the Hub's `GET /api/packs` items).
#[derive(Serialize, Deserialize, Clone)]
pub struct PackSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
}

#[derive(Deserialize)]
struct PacksEnvelope {
    packs: Vec<PackSummary>,
}

fn base(hub_url: &str) -> String {
    hub_url.trim_end_matches('/').to_string()
}

/// Shared client for every Hub call: stamps the launcher version so the Hub's
/// logs can tell clients (and versions) apart. Identification, not
/// authentication - the header is trivially forgeable and is treated as such.
pub(crate) fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(v) = reqwest::header::HeaderValue::from_str(env!("CARGO_PKG_VERSION")) {
        headers.insert("x-translocator-version", v);
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Browse every published pack with its latest version.
pub async fn list_packs(hub_url: &str) -> Result<Vec<PackSummary>, String> {
    let url = format!("{}/api/packs", base(hub_url));
    let resp = client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach the Hub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Hub returned {} for the pack list", resp.status()));
    }
    let env: PacksEnvelope = resp
        .json()
        .await
        .map_err(|e| format!("could not read the pack list: {e}"))?;
    Ok(env.packs)
}

/// Full pack record (publisher, links, timestamps). Display-only, so it rides
/// through as raw JSON rather than a strict struct.
pub async fn pack_detail(hub_url: &str, id: &str) -> Result<serde_json::Value, String> {
    let url = format!("{}/api/packs/{}", base(hub_url), id);
    let resp = client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach the Hub: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("pack not found".into());
    }
    if !resp.status().is_success() {
        return Err(format!("Hub returned {} for that pack", resp.status()));
    }
    resp.json()
        .await
        .map_err(|e| format!("could not read that pack: {e}"))
}

/// The manifest for a pack (latest, or a specific pinned version), so a Pack
/// page can list its mods and overrides.
pub async fn pack_manifest(
    hub_url: &str,
    id: &str,
    version: Option<&str>,
) -> Result<Manifest, String> {
    let mut url = format!("{}/api/packs/{}/manifest", base(hub_url), id);
    if let Some(v) = version.filter(|v| !v.is_empty()) {
        url.push_str(&format!("?version={v}"));
    }
    let resp = client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach the Hub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Hub returned {} for that manifest", resp.status()));
    }
    resp.json()
        .await
        .map_err(|e| format!("could not read that manifest: {e}"))
}
