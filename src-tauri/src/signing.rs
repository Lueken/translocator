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
// verify_strict is an inherent method on VerifyingKey, so the Verifier trait is
// deliberately not imported: importing it would offer a plain `verify` that
// accepts small-order keys.
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::store;

const KEY_FILE: &str = "signing.dat";
/// Must match the Hub's PAYLOAD_VERSION (canonical.ts) exactly. See that file
/// for what v3 changed and why.
pub const PAYLOAD_VERSION: &str = "translocator-pack-v3";

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

/// Decode a base64 raw 32-byte Ed25519 public key, as the Hub stores and
/// serves them. Rejects anything that is not exactly 32 bytes rather than
/// padding or truncating, because a key that is silently reshaped is a key
/// that verifies the wrong thing.
pub fn decode_public_key(public_key_b64: &str) -> Result<VerifyingKey, String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(public_key_b64.trim())
        .map_err(|e| format!("public key is not valid base64: {e}"))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| "public key is not 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| format!("public key is not a valid Ed25519 point: {e}"))
}

/// Fingerprint of a public key in the Hub's form, computed from the raw bytes
/// so it does not depend on how the key was encoded in transit.
pub fn fingerprint_of(key: &VerifyingKey) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("ed25519:{}", &hex[..32])
}

/// Does `signature_b64` verify `payload` under `public_key_b64`?
///
/// Returns a plain bool on purpose: every failure mode here means the same
/// thing to a caller (do not trust this document), and a Result invites
/// somebody to `unwrap_or(true)` or log the error and carry on. There is no
/// input for which this should return true by default, including malformed
/// base64, a wrong-length key, or a wrong-length signature.
pub fn verify(public_key_b64: &str, payload: &str, signature_b64: &str) -> bool {
    let Ok(key) = decode_public_key(public_key_b64) else {
        return false;
    };
    let Ok(raw_sig) = base64::engine::general_purpose::STANDARD.decode(signature_b64.trim()) else {
        return false;
    };
    let Ok(sig_bytes) = <[u8; 64]>::try_from(raw_sig) else {
        return false;
    };
    // verify_strict rejects signatures under small-order or torsion-component
    // public keys, which plain verify accepts. Those are the keys for which a
    // single signature can validate under more than one key, and we are about
    // to decide "did THIS publisher sign it" on the answer.
    key.verify_strict(payload.as_bytes(), &Signature::from_bytes(&sig_bytes))
        .is_ok()
}

/// Verify against a set of candidate keys, returning the fingerprint of the one
/// that matched. A publisher can have several device keys registered, and any
/// of them signing is legitimate.
pub fn verify_any(public_keys_b64: &[String], payload: &str, signature_b64: &str) -> Option<String> {
    for pk in public_keys_b64 {
        if verify(pk, payload, signature_b64) {
            return decode_public_key(pk).ok().map(|k| fingerprint_of(&k));
        }
    }
    None
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
    // v3: the sequence is signed. Absent is an error rather than a default,
    // because a defaulted zero would sign a revision counter that can never
    // increase, silently disabling the downgrade protection it exists for.
    let sequence = pack
        .get("sequence")
        .and_then(|v| v.as_u64())
        .ok_or("pack.sequence missing (v3 requires a monotonic sequence)")?;

    let mut entries: Vec<(u64, String, bool, String)> = Vec::with_capacity(mods.len());
    for m in mods {
        let fileid = m.get("fileid").and_then(|v| v.as_u64()).ok_or("mod missing fileid")?;
        let side = m.get("side").and_then(|v| v.as_str()).ok_or("mod missing side")?;
        let sha = m.get("sha256").and_then(|v| v.as_str()).ok_or("mod missing sha256")?;
        // Absent means required, matching the Hub: a manifest that omits the
        // flag must not quietly make a mod optional to a strict verifier.
        let required = m.get("required").and_then(|v| v.as_bool()).unwrap_or(true);
        entries.push((fileid, side.to_string(), required, sha.to_string()));
    }
    entries.sort_by_key(|e| e.0);

    let mut lines = vec![
        PAYLOAD_VERSION.to_string(),
        get("id")?.to_string(),
        get("version")?.to_string(),
        get("game_version")?.to_string(),
        if strict { "strict" } else { "open" }.to_string(),
        publisher_uid.to_string(),
        sequence.to_string(),
        manifest_digest(manifest),
    ];
    lines.extend(
        entries
            .into_iter()
            .map(|(f, s, r, h)| format!("{f}:{s}:{}:{h}", if r { "req" } else { "opt" })),
    );
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-implementation parity with the Hub, over a SHARED vector.
    ///
    /// This used to be a fixture invented here and a different fixture invented
    /// in the Hub, each asserting its own hand-pasted constant. Two such tests
    /// pass happily while the implementations drift apart, because neither one
    /// ever sees the other's bytes. Both suites now assert the same expected
    /// values over the same input: regenerate with
    /// `npx tsx scripts/print-vector.ts` in translocator-hub, and if only one
    /// side changes, that side is the one that broke.
    #[test]
    fn payload_matches_hub_implementation() {
        let manifest = shared_vector();
        assert_eq!(
            manifest_digest(&manifest),
            "be6fdfdb45e6b45b46d23774fb462e84f283f17d83f9fd151dee96688529790e",
            "canonical manifest digest diverged from the Hub"
        );
        let payload = signing_payload(&manifest, "uid-abc-123").unwrap();
        // Assert the literal bytes too, not only their hash: when this fails,
        // a diff of the payload says what diverged, where a hash mismatch only
        // says that something did.
        assert_eq!(
            payload,
            concat!(
                "translocator-pack-v3\n",
                "the-quire\n",
                "1.4.0\n",
                "1.22.6\n",
                "open\n",
                "uid-abc-123\n",
                "1786500000\n",
                "be6fdfdb45e6b45b46d23774fb462e84f283f17d83f9fd151dee96688529790e\n",
                "100:client:req:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
                "500:both:opt:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
                "900:server:req:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            ),
            "signed bytes diverged from the Hub"
        );
        let payload_sha: String = Sha256::digest(payload.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            payload_sha,
            "1c950024e83f4a66aa5949bfa37bfb920524bc49e41a8ef89071e417b829eac6",
            "signing payload diverged from the Hub"
        );
    }

    fn test_key(seed_byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed_byte; 32])
    }

    #[test]
    fn a_real_signature_verifies_and_nothing_else_does() {
        let key = test_key(7);
        let pk = public_key_b64(&key);
        let payload = signing_payload(&shared_vector(), "uid-abc-123").unwrap();
        let sig = sign(&key, &payload);

        assert!(verify(&pk, &payload, &sig), "a genuine signature must verify");

        // Every way this can go wrong must be false, not an error a caller can
        // shrug off. These are the cases an attacker actually supplies.
        let other = public_key_b64(&test_key(9));
        assert!(!verify(&other, &payload, &sig), "wrong key");
        assert!(!verify(&pk, &format!("{payload}\n"), &sig), "trailing newline is tampering");
        let mut swapped = payload.clone();
        swapped = swapped.replace("100:client:req:", "100:client:opt:");
        assert!(!verify(&pk, &swapped, &sig), "flipping required must break it");
        let tampered_hash = payload.replace("be6fdfdb", "be6fdfdc");
        assert!(!verify(&pk, &tampered_hash, &sig), "manifest digest is covered");
        let downgraded = payload.replace("1786500000", "1786499999");
        assert!(!verify(&pk, &downgraded, &sig), "sequence is covered");

        assert!(!verify("not base64!!", &payload, &sig), "malformed key");
        assert!(!verify(&pk, &payload, "not base64!!"), "malformed signature");
        assert!(!verify(&pk, &payload, ""), "empty signature");
        assert!(!verify("", &payload, &sig), "empty key");
        // A 31-byte key must be refused rather than padded into something.
        let short = b64(&[1u8; 31]);
        assert!(!verify(&short, &payload, &sig), "short key");
    }

    #[test]
    fn verify_any_finds_the_signing_key_among_several() {
        let signer = test_key(3);
        let payload = "anything at all";
        let sig = sign(&signer, payload);
        let keys = vec![
            public_key_b64(&test_key(1)),
            public_key_b64(&test_key(2)),
            public_key_b64(&signer),
        ];
        let matched = verify_any(&keys, payload, &sig).expect("one of the keys signed it");
        assert_eq!(matched, fingerprint(&signer), "reported the wrong key");

        let strangers = vec![public_key_b64(&test_key(1)), public_key_b64(&test_key(2))];
        assert!(verify_any(&strangers, payload, &sig).is_none(), "no key signed it");
        assert!(verify_any(&[], payload, &sig).is_none(), "an empty key list must never pass");
    }

    /// The two fingerprint helpers must agree, since one is used when
    /// registering a key and the other when reporting which key verified.
    #[test]
    fn fingerprints_agree_between_private_and_public_paths() {
        let key = test_key(42);
        let pk = decode_public_key(&public_key_b64(&key)).unwrap();
        assert_eq!(fingerprint(&key), fingerprint_of(&pk));
    }

    /// The shared v3 vector. Byte-for-byte the same input the Hub's
    /// scripts/print-vector.ts feeds its own canonicaliser.
    fn shared_vector() -> serde_json::Value {
        serde_json::json!({
            "manifest_version": 1,
            "pack": {
                "id": "the-quire",
                "name": "The Quire",
                "version": "1.4.0",
                "author": "Venah",
                "game_version": "1.22.6",
                "min_launcher_version": "0.1.0",
                "sequence": 1786500000
            },
            "mods": [
                { "modid": 99, "modidstr": "zeta", "name": "Zeta", "modversion": "2.0.0", "fileid": 900, "side": "server", "sha256": "c".repeat(64), "required": true },
                { "modid": 11, "modidstr": "alpha", "name": "Alpha", "modversion": "1.0.0", "fileid": 100, "side": "client", "sha256": "a".repeat(64), "required": true },
                { "modid": 55, "modidstr": "mid", "name": "Mid", "modversion": "1.5.0", "fileid": 500, "side": "both", "sha256": "b".repeat(64), "required": false }
            ]
        })
    }

    /// A missing sequence must be an error, never a defaulted zero: a zero
    /// signs a revision counter that can never increase, which silently
    /// disables the downgrade protection the field exists for.
    #[test]
    fn a_missing_sequence_is_refused() {
        let mut m = shared_vector();
        m["pack"].as_object_mut().unwrap().remove("sequence");
        let err = signing_payload(&m, "uid-abc-123").unwrap_err();
        assert!(err.contains("sequence"), "unhelpful error: {err}");
    }

    /// Omitting `required` must read as required on both sides. Defaulting the
    /// other way would quietly tell a strict verifier that a mandatory mod is
    /// optional.
    #[test]
    fn an_absent_required_flag_means_required() {
        let mut m = shared_vector();
        m["mods"][2].as_object_mut().unwrap().remove("required");
        let payload = signing_payload(&m, "uid-abc-123").unwrap();
        assert!(payload.contains("500:both:req:"), "absent required became optional");
    }
}
