//! Pack signing: the launcher-side half of `translocator-pack-v2`.
//!
//! The publisher's identity is their Vintage Story account; this module owns
//! the DEVICE key that signs on that account's behalf. The Ed25519 seed lives
//! DPAPI-sealed in `signing.dat` (like the account and saved servers) and never
//! leaves this machine; the Hub learns only the public key, bound to the VS
//! account at registration via a single-use mptoken.
//!
//! The canonical payload must be byte-identical to what the Hub rebuilds
//! (translocator-hub `src/lib/canonical.ts`, spec `docs/pack-signing.md`).
//! Parity is by construction: the manifest is serialized to a JSON value ONCE,
//! that exact value is what gets canonicalized for the digest AND sent in the
//! publish envelope, so the Hub canonicalizes the same document it received.

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::store;

const KEY_FILE: &str = "signing.dat";
const PAYLOAD_VERSION: &str = "translocator-pack-v2";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Load the device signing key, creating (and sealing) one on first use.
pub fn load_or_create_key(app: &AppHandle) -> Result<SigningKey, String> {
    if let Some(seed) = store::load_sealed(app, KEY_FILE) {
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| "signing.dat is corrupt (wrong seed length)".to_string())?;
        return Ok(SigningKey::from_bytes(&seed));
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("no OS randomness: {e}"))?;
    store::save_sealed(app, KEY_FILE, &seed)?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Whether a device key exists (without creating one).
pub fn has_key(app: &AppHandle) -> bool {
    store::load_sealed(app, KEY_FILE).is_some()
}

/// Base64 of the raw 32-byte public key, as the Hub stores it.
pub fn public_key_b64(key: &SigningKey) -> String {
    b64(key.verifying_key().as_bytes())
}

/// `ed25519:` + first 32 hex chars of sha256(raw public key). Matches the
/// Hub's fingerprint() exactly (computed from raw bytes, encoding-independent).
pub fn fingerprint(key: &SigningKey) -> String {
    let digest = Sha256::digest(key.verifying_key().as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("ed25519:{}", &hex[..32])
}

/// Sign arbitrary payload bytes; base64 of the 64-byte signature.
pub fn sign(key: &SigningKey, payload: &str) -> String {
    b64(&key.sign(payload.as_bytes()).to_bytes())
}

/// RFC 8785-style canonical JSON: object keys sorted, no insignificant
/// whitespace. Must match the Hub's canonicalJson (canonical.ts). Manifest
/// values are strings, integers, booleans, arrays, objects; no floats. Keys are
/// ASCII, so Rust's byte-wise sort and JS's UTF-16 sort agree.
pub fn canonical_json(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        // serde_json string escaping matches JSON.stringify for this data:
        // both escape `"` `\` and controls (with \b \t \n \f \r shortcuts),
        // both emit other characters raw.
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                canonical_json(&map[k.as_str()], out);
            }
            out.push('}');
        }
    }
}

/// sha256 (lowercase hex) over the canonical JSON of the manifest value.
pub fn manifest_digest(manifest: &serde_json::Value) -> String {
    let mut s = String::new();
    canonical_json(manifest, &mut s);
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The exact signed bytes. Line-oriented, LF, no trailing newline:
/// format id / pack id / version / game version / strict|open / publisher uid
/// / manifest sha256 / `fileid:side:sha256` sorted by fileid ascending.
pub fn signing_payload(manifest: &serde_json::Value, publisher_uid: &str) -> Result<String, String> {
    let pack = manifest.get("pack").ok_or("manifest has no pack block")?;
    let get = |k: &str| -> Result<&str, String> {
        pack.get(k)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("pack.{k} missing"))
    };
    let strict = pack.get("strict").and_then(|v| v.as_bool()).unwrap_or(false);

    let mods = manifest
        .get("mods")
        .and_then(|v| v.as_array())
        .ok_or("manifest has no mods array")?;
    let mut entries: Vec<(u64, String, String)> = Vec::with_capacity(mods.len());
    for m in mods {
        let fileid = m.get("fileid").and_then(|v| v.as_u64()).ok_or("mod missing fileid")?;
        let side = m.get("side").and_then(|v| v.as_str()).ok_or("mod missing side")?;
        let sha = m.get("sha256").and_then(|v| v.as_str()).ok_or("mod missing sha256")?;
        entries.push((fileid, side.to_string(), sha.to_string()));
    }
    entries.sort_by_key(|e| e.0);

    let mut lines = vec![
        PAYLOAD_VERSION.to_string(),
        get("id")?.to_string(),
        get("version")?.to_string(),
        get("game_version")?.to_string(),
        if strict { "strict" } else { "open" }.to_string(),
        publisher_uid.to_string(),
        manifest_digest(manifest),
    ];
    lines.extend(entries.into_iter().map(|(f, s, h)| format!("{f}:{s}:{h}")));
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-implementation parity with the Hub. The expected hashes are the
    /// ones printed by translocator-hub `scripts/test-signing.ts` for this
    /// exact manifest and uid; if this test fails, Rust and TypeScript no
    /// longer agree on the signed bytes and signatures will not verify.
    #[test]
    fn payload_matches_hub_implementation() {
        let manifest = serde_json::json!({
            "manifest_version": 1,
            "pack": {
                "id": "the-quire",
                "name": "The Quire",
                "version": "1.4.0",
                "author": "Venah",
                "game_version": "1.22.6",
                "min_launcher_version": "0.1.0"
            },
            "mods": [
                { "modid": 99, "modidstr": "zeta", "name": "Zeta", "modversion": "2.0.0", "fileid": 900, "side": "server", "sha256": "c".repeat(64), "required": true },
                { "modid": 11, "modidstr": "alpha", "name": "Alpha", "modversion": "1.0.0", "fileid": 100, "side": "client", "sha256": "a".repeat(64), "required": true },
                { "modid": 55, "modidstr": "mid", "name": "Mid", "modversion": "1.5.0", "fileid": 500, "side": "both", "sha256": "b".repeat(64), "required": false }
            ]
        });
        assert_eq!(
            manifest_digest(&manifest),
            "46d55c542c08344686492661a4fb9c44ba085cbe4707bb45cc47c6b4d73c3a16",
            "canonical manifest digest diverged from the Hub"
        );
        let payload = signing_payload(&manifest, "TestUid1234567890").unwrap();
        let payload_sha: String = Sha256::digest(payload.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            payload_sha,
            "c5f5178ef9e1cea12045cdd4b82b5774177f55ca5c3a233ffb777dec7e0e11bb",
            "signing payload diverged from the Hub"
        );
    }
}
