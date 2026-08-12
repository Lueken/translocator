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
    /// Monotonic revision counter, signed as part of the v3 payload: Unix
    /// seconds at the moment the manifest was curated. Stamped by
    /// `curate_pack`, never entered by hand.
    ///
    /// The publisher picks it rather than the Hub, so the Hub cannot decide
    /// which revision of a pack looks newest. An installed pack remembers the
    /// highest sequence it has seen and refuses anything lower, which is what
    /// stops a genuine older release being replayed as the current one.
    #[serde(default)]
    pub sequence: u64,
    /// Self-contained pack: the manifest defines the entire Mods folder.
    /// Spec default is false; signed into the pack payload, so omitted-when-
    /// false keeps the wire form and the signature consistent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Screenshot URLs for the pack page carousel (https-only, max 8).
    /// Signed like everything else in the manifest; changing the gallery is a
    /// pack-version bump by design.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gallery: Vec<String>,
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
    /// Absent means required, matching the Hub and the signing payload. A
    /// hard-required field here would turn an omitted flag into a parse failure
    /// on a manifest all three implementations otherwise agree about.
    #[serde(default = "yes")]
    pub required: bool,
}

fn yes() -> bool {
    true
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

fn is_false(b: &bool) -> bool {
    !*b
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
