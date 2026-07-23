//! The Translocator modpack manifest (schema v1.1), Rust side.
//! Canonical spec: `docs/modpack-manifest.md`. The curator produces this and the
//! install-pack flow consumes it; both round-trip it through the frontend, so it
//! is both Serialize and Deserialize.

use serde::{Deserialize, Serialize};

/// Highest manifest_version this launcher produces/understands.
pub const MANIFEST_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone)]
pub struct ModDbRef {
    pub modid: Option<u64>,
    pub assetid: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestPack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub game_version: String,
    pub min_launcher_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moddb: Option<ModDbRef>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ManifestLinks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub donate: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestServer {
    pub address: String,
    #[serde(default)]
    pub auto_add: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestMod {
    pub modid: u64,
    pub modidstr: String,
    pub name: String,
    pub modversion: String,
    pub fileid: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub releaseid: Option<u64>,
    pub side: String,
    pub sha256: String,
    pub required: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestOverride {
    pub path: String,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    pub content: String,
}

fn default_encoding() -> String {
    "utf8".into()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub manifest_version: u32,
    pub pack: ManifestPack,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<ManifestLinks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ManifestServer>,
    pub mods: Vec<ManifestMod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<ManifestOverride>,
}
