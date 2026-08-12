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
    pack_manifest_document(hub_url, id, version).await.map(|(_, m)| m)
}

/// The manifest as BOTH the document the Hub actually served and the typed view
/// of it.
///
/// Signature verification has to run over the served document, never over a
/// re-serialized `Manifest`. `Manifest` models the fields the launcher acts on
/// and silently drops the rest, so a digest taken from what we kept would attest
/// to a different document than the one the publisher signed: any field outside
/// the struct could be changed in flight without disturbing the signature. The
/// typed view is derived FROM the same value, so everything the launcher acts on
/// is covered by whatever the digest covers.
pub async fn pack_manifest_document(
    hub_url: &str,
    id: &str,
    version: Option<&str>,
) -> Result<(serde_json::Value, Manifest), String> {
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
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("could not read that manifest: {e}"))?;
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| format!("could not read that manifest: {e}"))?;
    let typed: Manifest = serde_json::from_value(value.clone())
        .map_err(|e| format!("could not read that manifest: {e}"))?;
    Ok((value, typed))
}

/// The pack's signed verification document, pinned to one version.
///
/// Deliberately pinned rather than latest: the signature covers a specific
/// version, and pairing a freshly published manifest with the previous
/// version's document would fail verification for a reason that has nothing to
/// do with the pack being untrustworthy.
pub async fn pack_verification(
    hub_url: &str,
    id: &str,
    version: &str,
) -> Result<crate::pack_verify::VerificationDoc, String> {
    let url = format!("{}/api/packs/{}/verify?version={}", base(hub_url), id, version);
    let resp = client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach the Hub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "the Hub returned {} instead of a signature for {id} {version}, so there is no way to \
             tell who published it. Nothing was installed.",
            resp.status()
        ));
    }
    resp.json()
        .await
        .map_err(|e| format!("could not read the signature for that pack: {e}"))
}
