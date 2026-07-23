//! Server browser.
//!
//! Phase 1 is the Public tab: the Vintage Story masterserver publishes every
//! server that opts into the public list, and we mirror it here so a player can
//! find one without leaving the launcher. Joining (launch an installation with
//! `--connect`) and the Private saved-server store land in later phases.
//!
//! The webview CSP forbids direct network calls, so this runs Rust-side.

use crate::store;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

const MASTER_LIST: &str = "https://masterserver.vintagestory.at/api/v1/servers/list";
// Saved private servers, DPAPI-sealed (they can carry a server password).
const PRIVATE_FILE: &str = "servers.dat";

/// One public server, trimmed to what the browser row shows.
#[derive(Serialize, Clone)]
pub struct PublicServer {
    pub name: String,
    pub address: String,
    pub players: i64,
    pub max_players: i64,
    pub game_version: String,
    pub mod_count: usize,
    pub has_password: bool,
    pub whitelisted: bool,
    pub playstyle: String,
    pub description: String,
}

// The masterserver payload. `maxPlayers`/`players` come back as either a string
// or a number depending on the server, so they ride as raw JSON and get coerced.
#[derive(Deserialize)]
struct RawServer {
    #[serde(rename = "serverName", default)]
    name: String,
    #[serde(rename = "serverIP", default)]
    address: String,
    #[serde(default)]
    mods: Vec<serde_json::Value>,
    #[serde(rename = "maxPlayers", default)]
    max_players: serde_json::Value,
    #[serde(default)]
    players: serde_json::Value,
    #[serde(rename = "gameVersion", default)]
    game_version: String,
    #[serde(rename = "hasPassword", default)]
    has_password: bool,
    #[serde(default)]
    whitelisted: bool,
    #[serde(rename = "gameDescription", default)]
    description: String,
    #[serde(default)]
    playstyle: Option<RawPlaystyle>,
}

#[derive(Deserialize)]
struct RawPlaystyle {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    data: Vec<RawServer>,
}

/// Coerce a string-or-number JSON value to an i64 (0 on anything unexpected).
fn as_i64(v: &serde_json::Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or(0)
}

fn map_server(r: RawServer) -> PublicServer {
    PublicServer {
        name: r.name,
        address: r.address,
        players: as_i64(&r.players),
        max_players: as_i64(&r.max_players),
        game_version: r.game_version,
        mod_count: r.mods.len(),
        has_password: r.has_password,
        whitelisted: r.whitelisted,
        playstyle: r.playstyle.map(|p| p.id).unwrap_or_default(),
        description: r.description,
    }
}

/// Fetch the public server list from the masterserver. Servers with no address
/// are dropped (they can't be joined). Order is left as the masterserver sends
/// it; the UI does its own sort/filter.
pub async fn list_public_servers() -> Result<Vec<PublicServer>, String> {
    let resp = reqwest::Client::new()
        .get(MASTER_LIST)
        .send()
        .await
        .map_err(|e| format!("could not reach the server list: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("masterserver returned {}", resp.status()));
    }
    let env: Envelope = resp
        .json()
        .await
        .map_err(|e| format!("could not read the server list: {e}"))?;
    Ok(env
        .data
        .into_iter()
        .filter(|r| !r.address.trim().is_empty())
        .map(map_server)
        .collect())
}

// ---- Private (saved) servers ----

/// A server the user saved by hand: pinned to an installation, optionally with a
/// password. Not part of the public list. The `id` is minted by the frontend so
/// edits upsert cleanly.
#[derive(Serialize, Deserialize, Clone)]
pub struct PrivateServer {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub install_path: String,
}

pub fn list_private(app: &AppHandle) -> Vec<PrivateServer> {
    store::load_sealed(app, PRIVATE_FILE)
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_private(app: &AppHandle, list: &[PrivateServer]) -> Result<(), String> {
    let bytes = serde_json::to_vec(list).map_err(|e| e.to_string())?;
    store::save_sealed(app, PRIVATE_FILE, &bytes)
}

/// Add a new saved server, or update the existing one with the same `id`.
/// Returns the full list so the UI can refresh in one round trip.
pub fn upsert_private(app: &AppHandle, mut s: PrivateServer) -> Result<Vec<PrivateServer>, String> {
    s.address = s.address.trim().to_string();
    if s.address.is_empty() {
        return Err("server address is empty".into());
    }
    if s.id.trim().is_empty() {
        return Err("server id is missing".into());
    }
    let mut list = list_private(app);
    match list.iter_mut().find(|x| x.id == s.id) {
        Some(existing) => *existing = s,
        None => list.push(s),
    }
    save_private(app, &list)?;
    Ok(list)
}

pub fn remove_private(app: &AppHandle, id: &str) -> Result<Vec<PrivateServer>, String> {
    let mut list = list_private(app);
    list.retain(|x| x.id != id);
    save_private(app, &list)?;
    Ok(list)
}
