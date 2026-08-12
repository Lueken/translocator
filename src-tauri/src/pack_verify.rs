//! Deciding whether a pack really came from whoever the Hub says it did.
//!
//! Two checks, and neither one is sufficient alone.
//!
//! The first is arithmetic: the Hub's verification document carries a signature,
//! and the manifest it describes can be turned back into the exact bytes that
//! were signed. Either the signature checks out under one of the publisher's
//! keys or it does not. This catches a manifest altered in transit, a manifest
//! altered in the Hub's own database, and a document assembled from parts of
//! two different releases.
//!
//! What it cannot catch is the document being a perfectly valid signature by
//! somebody else. The document names the keys it wants to be checked against,
//! so anyone able to serve it can name their own key and sign with it, and the
//! arithmetic will be flawless. That is not a flaw in the signature, it is what
//! a signature is: proof that a key signed something, not proof that the key
//! belonged to anyone you meant to trust.
//!
//! So the second check is memory. The first install of a pack records who
//! signed it, and every install afterwards has to match. The anchor is the
//! user's own earlier decision, which is the one thing in the exchange that the
//! Hub does not get to choose. It is trust on first use, with the honesty that
//! implies: the first install is a leap, and every one after it is checked.

use serde::Deserialize;

use crate::signing;

/// One key the Hub says may sign for this pack.
///
/// The Hub also sends a `fingerprint` per key, deliberately not modelled here:
/// a fingerprint is a function of the public key, so reading the Hub's copy
/// instead of computing our own would mean trusting a label over the thing it
/// labels.
#[derive(Deserialize, Clone, Default)]
pub struct AuthorizedKey {
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub public_key: String,
}

/// The Hub's account of who signed, which the signature then has to bear out.
#[derive(Deserialize, Clone, Default)]
pub struct Provenance {
    #[serde(default)]
    pub signed_by: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub key_fingerprint: Option<String>,
}

/// `GET /api/packs/:id/verify`.
///
/// Every field is optional at the serde level on purpose. This document arrives
/// from the network and the whole point of the module is to distrust it, so a
/// missing field has to become a refusal we can word, not a deserialization
/// error that reads like the Hub is down.
#[derive(Deserialize, Clone, Default)]
pub struct VerificationDoc {
    #[serde(default)]
    pub payload_version: String,
    #[serde(default)]
    pub pack: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub authorized_keys: Vec<AuthorizedKey>,
    #[serde(default)]
    pub signature: Option<String>,
}

/// What a passing verification establishes. Only ever built by [`verify_pack`],
/// so holding one means the signature checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPack {
    /// VS account uid named in the signed payload.
    pub uid: String,
    /// Fingerprint of the key that actually verified, derived from that key
    /// rather than read out of the document.
    pub key_fingerprint: String,
    /// Signed revision counter, for the downgrade check.
    pub sequence: u64,
    /// Display name, for telling the user who they are trusting. Not signed.
    pub publisher_name: String,
}

/// Check a verification document against the manifest it claims to describe.
///
/// `manifest_value` must be the JSON the Hub served, not a struct serialized
/// back out. See `hub::pack_manifest_document`.
pub fn verify_pack(
    manifest_value: &serde_json::Value,
    doc: &VerificationDoc,
    pack_id: &str,
    pack_version: &str,
) -> Result<VerifiedPack, String> {
    if doc.payload_version != signing::PAYLOAD_VERSION {
        return Err(format!(
            "this pack is signed in the '{}' format and this launcher verifies '{}'. \
             Update Translocator, or ask the publisher to republish.",
            if doc.payload_version.is_empty() { "unknown" } else { &doc.payload_version },
            signing::PAYLOAD_VERSION
        ));
    }
    // The document has to be about the pack we asked for. Without this a Hub
    // could answer any request with a genuine signature over some other pack.
    if doc.pack != pack_id || doc.version != pack_version {
        return Err(format!(
            "the Hub signed '{} {}' when asked about '{pack_id} {pack_version}'. Nothing was installed.",
            doc.pack, doc.version
        ));
    }

    let uid = doc.provenance.uid.as_deref().unwrap_or("").trim();
    if uid.is_empty() {
        return Err("this pack version is not signed by any account, so there is no way to tell \
                    who published it. Nothing was installed."
            .into());
    }
    let signature = doc.signature.as_deref().unwrap_or("").trim();
    if signature.is_empty() {
        return Err("this pack version carries no signature, so there is no way to tell who \
                    published it. Nothing was installed."
            .into());
    }

    // Only the named signer's own keys are candidates. Accepting any team key
    // would let one collaborator sign a payload naming a different member's
    // uid: the arithmetic would pass and we would then pin an account to a
    // fingerprint that never belonged to it.
    let candidates: Vec<String> = doc
        .authorized_keys
        .iter()
        .filter(|k| k.uid == uid && !k.public_key.trim().is_empty())
        .map(|k| k.public_key.trim().to_string())
        .collect();
    if candidates.is_empty() {
        return Err(format!(
            "the Hub says '{pack_id} {pack_version}' was signed by an account with no registered \
             signing key, which cannot be true. Nothing was installed."
        ));
    }

    // The signed bytes, rebuilt from the manifest itself. Every field the
    // launcher will act on is folded into the manifest digest inside this
    // payload, so a manifest that differs anywhere fails here.
    let payload = signing::signing_payload(manifest_value, uid)?;

    let key_fingerprint = signing::verify_any(&candidates, &payload, signature).ok_or_else(|| {
        format!(
            "the signature on '{pack_id} {pack_version}' does not match its contents. Either the \
             manifest was altered after it was signed, or it was signed by someone other than the \
             account it names. Nothing was installed."
        )
    })?;

    // The Hub's own claim about which key signed must agree with the key that
    // demonstrably did. A document whose self-description contradicts its own
    // signature is malformed, whatever the reason, and pinning from it would
    // record something no party actually asserted.
    if let Some(claimed) = doc.provenance.key_fingerprint.as_deref().map(str::trim) {
        if !claimed.is_empty() && claimed != key_fingerprint {
            return Err(format!(
                "the Hub credits key {claimed} for this pack but {key_fingerprint} is what signed \
                 it. Nothing was installed."
            ));
        }
    }

    let sequence = manifest_value
        .get("pack")
        .and_then(|p| p.get("sequence"))
        .and_then(|v| v.as_u64())
        .ok_or("this pack has no revision sequence, so a superseded release could be replayed \
                as the current one. Nothing was installed.")?;

    Ok(VerifiedPack {
        uid: uid.to_string(),
        key_fingerprint,
        sequence,
        publisher_name: doc
            .authorized_keys
            .iter()
            .find(|k| k.uid == uid && !k.publisher.is_empty())
            .map(|k| k.publisher.clone())
            .or_else(|| doc.provenance.signed_by.clone())
            .unwrap_or_default(),
    })
}

/// What an installation already believes about this pack's publisher.
///
/// All three empty/zero means nothing is pinned yet: a first install, or one
/// made by a launcher that predates pinning. Both are trust-on-first-use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pins {
    pub uid: String,
    pub key_fingerprint: String,
    pub sequence: u64,
}

/// Does a verified publisher match what this pack was installed with before?
///
/// The fields are checked independently rather than as a set, so a pin written
/// by an older launcher that recorded only some of them still constrains what
/// it did record.
pub fn check_pins(pins: &Pins, v: &VerifiedPack) -> Result<(), String> {
    if !pins.uid.is_empty() && pins.uid != v.uid {
        return Err(format!(
            "this pack was installed from account {} and is now signed by {}. A pack changing \
             hands can be legitimate, but it looks identical to someone taking over the pack id, \
             so Translocator will not do it silently. Remove the existing installation of this \
             pack to accept the new publisher.",
            pins.uid, v.uid
        ));
    }
    if !pins.key_fingerprint.is_empty() && pins.key_fingerprint != v.key_fingerprint {
        return Err(format!(
            "this pack was installed under signing key {} and is now signed by {}. That is what a \
             publisher's new machine looks like, and also what a stolen key looks like. Confirm \
             the new key with the publisher, then remove the existing installation to accept it.",
            pins.key_fingerprint, v.key_fingerprint
        ));
    }
    // Equal is fine: reinstalling the same release is not a downgrade.
    if v.sequence < pins.sequence {
        return Err(format!(
            "the Hub is offering revision {} of this pack when {} has already been seen. This is \
             an older release being served as the current one. Nothing was installed.",
            v.sequence, pins.sequence
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn manifest(sequence: u64) -> serde_json::Value {
        serde_json::json!({
            "manifest_version": 1,
            "pack": {
                "id": "the-quire",
                "name": "The Quire",
                "version": "1.4.0",
                "author": "Venah",
                "game_version": "1.22.6",
                "min_launcher_version": "0.1.0",
                "sequence": sequence
            },
            "mods": [
                { "modid": 11, "modidstr": "alpha", "name": "Alpha", "modversion": "1.0.0",
                  "fileid": 100, "side": "client", "sha256": "a".repeat(64), "required": true }
            ]
        })
    }

    /// A document the Hub would actually serve for `manifest(seq)`, signed by
    /// `signer` on behalf of `uid`.
    fn doc_for(m: &serde_json::Value, signer: &SigningKey, uid: &str) -> VerificationDoc {
        let payload = signing::signing_payload(m, uid).unwrap();
        VerificationDoc {
            payload_version: signing::PAYLOAD_VERSION.to_string(),
            pack: "the-quire".into(),
            version: "1.4.0".into(),
            provenance: Provenance {
                signed_by: Some("Venah".into()),
                uid: Some(uid.into()),
                key_fingerprint: Some(signing::fingerprint(signer)),
            },
            authorized_keys: vec![AuthorizedKey {
                publisher: "Venah".into(),
                uid: uid.into(),
                public_key: signing::public_key_b64(signer),
            }],
            signature: Some(signing::sign(signer, &payload)),
        }
    }

    #[test]
    fn a_genuine_document_verifies() {
        let m = manifest(1786500000);
        let signer = key(7);
        let v = verify_pack(&m, &doc_for(&m, &signer, "uid-venah"), "the-quire", "1.4.0").unwrap();
        assert_eq!(v.uid, "uid-venah");
        assert_eq!(v.key_fingerprint, signing::fingerprint(&signer));
        assert_eq!(v.sequence, 1786500000);
        assert_eq!(v.publisher_name, "Venah");
    }

    /// Every one of these is a document an attacker can actually serve. None of
    /// them may return Ok, and none may return Ok by falling through a check
    /// that was skipped because a field was absent.
    #[test]
    fn nothing_short_of_a_real_signature_passes() {
        let m = manifest(1786500000);
        let signer = key(7);
        let good = doc_for(&m, &signer, "uid-venah");

        let mut wrong_pack = good.clone();
        wrong_pack.pack = "some-other-pack".into();
        assert!(verify_pack(&m, &wrong_pack, "the-quire", "1.4.0").is_err(), "wrong pack id");

        let mut wrong_version = good.clone();
        wrong_version.version = "1.3.0".into();
        assert!(verify_pack(&m, &wrong_version, "the-quire", "1.4.0").is_err(), "wrong version");

        let mut old_format = good.clone();
        old_format.payload_version = "translocator-pack-v2".into();
        assert!(verify_pack(&m, &old_format, "the-quire", "1.4.0").is_err(), "v2 document");
        let mut no_format = good.clone();
        no_format.payload_version = String::new();
        assert!(verify_pack(&m, &no_format, "the-quire", "1.4.0").is_err(), "no format id");

        let mut no_sig = good.clone();
        no_sig.signature = None;
        assert!(verify_pack(&m, &no_sig, "the-quire", "1.4.0").is_err(), "absent signature");
        let mut blank_sig = good.clone();
        blank_sig.signature = Some("   ".into());
        assert!(verify_pack(&m, &blank_sig, "the-quire", "1.4.0").is_err(), "blank signature");

        let mut no_uid = good.clone();
        no_uid.provenance.uid = None;
        assert!(verify_pack(&m, &no_uid, "the-quire", "1.4.0").is_err(), "absent uid");

        let mut no_keys = good.clone();
        no_keys.authorized_keys.clear();
        assert!(verify_pack(&m, &no_keys, "the-quire", "1.4.0").is_err(), "empty key list");

        // The manifest changed after signing: one byte of one hash.
        let mut tampered = manifest(1786500000);
        tampered["mods"][0]["sha256"] = serde_json::json!("b".repeat(64));
        assert!(verify_pack(&tampered, &good, "the-quire", "1.4.0").is_err(), "altered mod hash");

        // A field the typed Manifest does not model still moves the digest,
        // which is the whole reason verification runs over the served document.
        let mut extra = manifest(1786500000);
        extra["mods"][0]["surprise"] = serde_json::json!("payload");
        assert!(verify_pack(&extra, &good, "the-quire", "1.4.0").is_err(), "unmodelled field");

        // Replaying a genuine signature over a bumped sequence.
        let bumped = manifest(1786599999);
        assert!(verify_pack(&bumped, &good, "the-quire", "1.4.0").is_err(), "sequence swapped");

        let mut no_sequence = manifest(1786500000);
        no_sequence["pack"].as_object_mut().unwrap().remove("sequence");
        assert!(verify_pack(&no_sequence, &good, "the-quire", "1.4.0").is_err(), "no sequence");
    }

    /// The document names its own key list, so this is the attack it invites:
    /// a stranger signs the payload and lists their own key as authorized. The
    /// arithmetic passes, and only the pin stops it.
    #[test]
    fn a_stranger_signing_for_themselves_passes_verification_and_fails_the_pin() {
        let m = manifest(1786500000);
        let venah = key(7);
        let thief = key(9);

        let forged = doc_for(&m, &thief, "uid-thief");
        let v = verify_pack(&m, &forged, "the-quire", "1.4.0")
            .expect("a self-consistent document verifies; that is the point of the pin");
        assert_eq!(v.uid, "uid-thief");

        let pinned = Pins {
            uid: "uid-venah".into(),
            key_fingerprint: signing::fingerprint(&venah),
            sequence: 1786400000,
        };
        let err = check_pins(&pinned, &v).unwrap_err();
        assert!(err.contains("uid-venah"), "unhelpful error: {err}");
    }

    /// A team-mate signing a payload that names somebody else's uid must not
    /// verify, or we would pin an account to a key it never owned.
    #[test]
    fn one_publishers_key_cannot_sign_for_another() {
        let m = manifest(1786500000);
        let venah = key(7);
        let mate = key(8);

        let payload = signing::signing_payload(&m, "uid-venah").unwrap();
        let mut doc = doc_for(&m, &venah, "uid-venah");
        // Mate signs Venah's payload, and the Hub lists both team keys.
        doc.signature = Some(signing::sign(&mate, &payload));
        doc.provenance.key_fingerprint = None;
        doc.authorized_keys.push(AuthorizedKey {
            publisher: "Mate".into(),
            uid: "uid-mate".into(),
            public_key: signing::public_key_b64(&mate),
        });
        assert!(
            verify_pack(&m, &doc, "the-quire", "1.4.0").is_err(),
            "a key belonging to a different uid must not satisfy this payload"
        );
    }

    /// A publisher's second machine is a real thing and it still has to stop.
    /// The refusal is the feature: it is indistinguishable from a stolen key.
    #[test]
    fn a_new_key_for_the_same_account_stops() {
        let m = manifest(1786500000);
        let second_machine = key(11);
        let v = verify_pack(&m, &doc_for(&m, &second_machine, "uid-venah"), "the-quire", "1.4.0")
            .unwrap();
        let pinned = Pins {
            uid: "uid-venah".into(),
            key_fingerprint: signing::fingerprint(&key(7)),
            sequence: 1786400000,
        };
        assert!(check_pins(&pinned, &v).is_err(), "a changed device key must stop the install");
    }

    #[test]
    fn pins_allow_the_same_release_and_refuse_an_older_one() {
        let signer = key(7);
        let pinned = Pins {
            uid: "uid-venah".into(),
            key_fingerprint: signing::fingerprint(&signer),
            sequence: 1786500000,
        };
        let same = manifest(1786500000);
        let v = verify_pack(&same, &doc_for(&same, &signer, "uid-venah"), "the-quire", "1.4.0").unwrap();
        assert!(check_pins(&pinned, &v).is_ok(), "reinstalling the same release is not a downgrade");

        let newer = manifest(1786600000);
        let v = verify_pack(&newer, &doc_for(&newer, &signer, "uid-venah"), "the-quire", "1.4.0").unwrap();
        assert!(check_pins(&pinned, &v).is_ok(), "a newer release must install");

        let older = manifest(1786400000);
        let v = verify_pack(&older, &doc_for(&older, &signer, "uid-venah"), "the-quire", "1.4.0").unwrap();
        let err = check_pins(&pinned, &v).unwrap_err();
        assert!(err.contains("older release"), "unhelpful error: {err}");
    }

    #[test]
    fn nothing_pinned_yet_is_trust_on_first_use() {
        let m = manifest(1786500000);
        let v = verify_pack(&m, &doc_for(&m, &key(7), "uid-venah"), "the-quire", "1.4.0").unwrap();
        assert!(check_pins(&Pins::default(), &v).is_ok());
    }

    /// A pin written by a launcher that recorded only some fields must still
    /// enforce the fields it did record.
    #[test]
    fn a_partial_pin_still_constrains_what_it_holds() {
        let m = manifest(1786500000);
        let v = verify_pack(&m, &doc_for(&m, &key(7), "uid-venah"), "the-quire", "1.4.0").unwrap();
        let uid_only = Pins { uid: "uid-someone-else".into(), ..Pins::default() };
        assert!(check_pins(&uid_only, &v).is_err(), "uid alone must still be enforced");
        let seq_only = Pins { sequence: u64::MAX, ..Pins::default() };
        assert!(check_pins(&seq_only, &v).is_err(), "sequence alone must still be enforced");
    }
}
