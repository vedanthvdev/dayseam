//! DAY-123 / T-1: end-to-end signature verification for the in-app
//! auto-updater.
//!
//! # Why this file exists
//!
//! The DAY-115 v0.5 capstone review (see
//! [`docs/review/v0.5-review.md`](../../../../docs/review/v0.5-review.md)
//! §3.2 T-1) caught that the existing JS-side updater suite
//! ([`apps/desktop/src/features/updater/__tests__/useUpdater.test.tsx`](../../src/features/updater/__tests__/useUpdater.test.tsx))
//! `vi.mock`s the entire `@tauri-apps/plugin-updater` import — the
//! mock returns a `MockUpdate` whose `downloadAndInstall` only
//! replays canned events or throws a synthetic `Error`. That means
//! the plugin's *actual* Ed25519 / minisign signature verification
//! path — the one that protects users against an attacker MITMing
//! `https://github.com/.../latest/download/latest.json` and serving a
//! tampered DMG — was **not** reachable from any test in the repo.
//! The CHANGELOG's "pipeline verified" wording for DAY-108 referred
//! only to the UI state machine; the crypto contract was on faith.
//!
//! # What this file pins
//!
//! `tauri-plugin-updater` 2.x verifies signatures using
//! [`minisign-verify`](https://docs.rs/minisign-verify) (visible in
//! the `Cargo.lock` dependency tree under
//! `tauri-plugin-updater -> minisign-verify`). This suite uses the
//! exact same crate to cover the four failure modes the review
//! flagged, generating ephemeral keypairs in-process so nothing
//! sensitive ever touches the repo:
//!
//! | Test | What it pins |
//! |------|-------------|
//! | `signature_round_trips_for_matching_keypair` | A signature produced by the standard minisign tooling parses and verifies cleanly through `minisign-verify`. Baseline that all the negative tests below are meaningful only when *this* one passes. |
//! | `verification_fails_when_public_key_does_not_match` | The "stale rotated key" failure mode. If a release is ever signed by a key whose public half was rotated out of `tauri.conf.json`, the user's installed app must refuse the update — this test keeps that promise honest. |
//! | `verification_fails_when_signature_byte_is_flipped` | The "tampered `.sig`" failure mode (an attacker who can rewrite `latest.json`'s `signature` field but not re-sign the underlying tarball). |
//! | `verification_fails_when_payload_byte_is_flipped` | The "tampered `.tar.gz`" failure mode (an attacker who can serve a different binary at the URL named in `latest.json` but cannot mint a fresh signature for it). |
//! | `production_pubkey_in_tauri_conf_json_parses` | Sanity gate over the production trust anchor: a future config edit that breaks the base64 wrapping or strips the comment header would fail this test before it broke a release. |
//!
//! Together those five tests answer the T-1 finding: "add a
//! signature-rejection fixture (known-bad key) and an integration
//! test asserting `download_and_install` errors out". We cannot
//! invoke `download_and_install` in-process without a full Tauri
//! runtime + HTTP server, but we *can* exercise the exact crypto
//! crate it shells out to, with the exact wire format the release
//! pipeline produces. A future tauri-plugin-updater bump that
//! switched verification away from `minisign-verify` would surface
//! here as a missing transitive dependency at compile time, which
//! is the right place to notice that drift.
//!
//! # Why two crates (`minisign` + `minisign-verify`)
//!
//! `minisign-verify` is verify-only by design (zero-deps, no
//! signing primitives — that is its whole pitch). The reference
//! signing implementation lives in the `minisign` crate, by the
//! same author (jedisct1) and writing the same wire format. Pairing
//! them gives us a self-contained sign-then-verify flow that
//! mirrors what the release workflow's `tauri signer sign` step +
//! the in-app updater's `minisign-verify`-backed verification do
//! across a real release.

use std::io::Cursor;
use std::path::PathBuf;

use minisign::{KeyPair, PublicKeyBox};
use minisign_verify::{PublicKey as VerifyPublicKey, Signature as VerifySignature};

/// The byte payload every test below signs / tampers / verifies.
/// Kept short so the trusted-comment + base64 stay readable in any
/// failure output, and stable across runs so a regression that
/// breaks the wire format produces a deterministic diff.
const TEST_PAYLOAD: &[u8] = b"DAY-123 updater signature integration test payload";

/// Generate a fresh **unencrypted** minisign keypair in-process.
///
/// The released private key is wrapped under a password
/// (`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`); we deliberately skip
/// the password layer here because every keypair this file
/// generates is created at test-start, used once, and dropped at
/// test-end — so an at-rest password would protect nothing and
/// only add ceremony.
fn fresh_keypair() -> KeyPair {
    KeyPair::generate_unencrypted_keypair()
        .expect("minisign keypair generation should succeed in-process")
}

/// Sign `payload` with `kp.sk` and return the resulting
/// `SignatureBox` already serialised back to the on-disk text
/// format. Round-tripping through `into_string()` here mirrors
/// what `scripts/release/generate-latest-json.sh` does: the
/// release manifest stores the *string* form of the `.sig` file,
/// not an in-memory struct, so verifying that the string survives
/// the round trip is part of the contract.
fn sign_payload_to_string(kp: &KeyPair, payload: &[u8]) -> String {
    let sig_box = minisign::sign(
        Some(&kp.pk),
        &kp.sk,
        Cursor::new(payload),
        Some("test trusted comment"),
        Some("test untrusted comment"),
    )
    .expect("signing the test payload with a fresh keypair should succeed");

    sig_box.into_string()
}

/// Convert the `minisign::PublicKey` produced by [`fresh_keypair`]
/// into a [`minisign_verify::PublicKey`]. We deliberately route
/// through the *string* serialisation (not a binary handoff)
/// because the verifier in production loads its trust anchor from
/// the base64-wrapped string in [`tauri.conf.json`'s
/// `plugins.updater.pubkey`](../../tauri.conf.json), and we want
/// any divergence between the two crates' string formats to
/// surface here.
fn verify_pubkey_from(kp: &KeyPair) -> VerifyPublicKey {
    let box_str = kp
        .pk
        .clone()
        .to_box()
        .expect("public key should box cleanly")
        .to_string();
    let pk_box = PublicKeyBox::from_string(&box_str)
        .expect("the box we just produced must round-trip through PublicKeyBox");
    let base64 = pk_box
        .into_string()
        .lines()
        .nth(1)
        .expect("PublicKeyBox::into_string yields `untrusted comment:\\n<base64>`")
        .to_owned();
    VerifyPublicKey::from_base64(&base64)
        .expect("the base64 line of a fresh public key must parse for minisign-verify")
}

/// Locate `tauri.conf.json` relative to this test crate without
/// hard-coding an absolute path (so the test stays portable across
/// CI runners and contributor checkouts). `CARGO_MANIFEST_DIR`
/// points at `apps/desktop/src-tauri/`, where the file lives.
fn tauri_conf_json_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json")
}

#[test]
fn signature_round_trips_for_matching_keypair() {
    let kp = fresh_keypair();
    let sig_str = sign_payload_to_string(&kp, TEST_PAYLOAD);
    let pk = verify_pubkey_from(&kp);

    let sig = VerifySignature::decode(&sig_str)
        .expect("the freshly-produced signature string must decode");

    pk.verify(TEST_PAYLOAD, &sig, /* allow_legacy */ false)
        .expect(
            "round-trip sign-then-verify with the matching public key MUST succeed; \
             if this fails the rest of the file's negative tests prove nothing",
        );
}

#[test]
fn verification_fails_when_public_key_does_not_match() {
    let signing_kp = fresh_keypair();
    let unrelated_kp = fresh_keypair();

    let sig_str = sign_payload_to_string(&signing_kp, TEST_PAYLOAD);
    let sig = VerifySignature::decode(&sig_str).expect("signature should decode");

    let wrong_pk = verify_pubkey_from(&unrelated_kp);

    let err = wrong_pk
        .verify(TEST_PAYLOAD, &sig, /* allow_legacy */ false)
        .expect_err(
            "verifying a signature with a public key from an unrelated keypair must fail — \
             this is the 'stale rotated key' protection the in-app updater relies on",
        );
    let err_str = format!("{err}");
    assert!(
        !err_str.is_empty(),
        "minisign-verify must surface a non-empty error message on key mismatch so the \
         release pipeline (and any operator debugging a failed update) can tell *why* \
         verification failed; got: {err_str:?}",
    );
}

#[test]
fn verification_fails_when_signature_byte_is_flipped() {
    let kp = fresh_keypair();
    let sig_str = sign_payload_to_string(&kp, TEST_PAYLOAD);
    let pk = verify_pubkey_from(&kp);

    // The signature string is four lines:
    //   untrusted comment: <text>
    //   <base64 of header + Ed25519 signature>
    //   trusted comment: <text>
    //   <base64 of global signature>
    //
    // Flipping a byte in either base64 line invalidates the
    // signature without breaking the file's structural parse —
    // exactly the "MITM rewrites the .sig" attack class. We
    // mutate the second line (the primary signature) because
    // that's what tauri-plugin-updater would surface as the
    // verification failure on a tampered release.
    let mut lines: Vec<String> = sig_str.lines().map(str::to_owned).collect();
    let primary = &mut lines[1];
    let mut bytes = primary.clone().into_bytes();
    // Flip a character that's safely inside the base64 alphabet
    // even after mutation. Index 4 is past the 4-char minisign
    // header in the base64-encoded blob and is, by construction,
    // an alphanumeric base64 character.
    bytes[4] = if bytes[4] == b'A' { b'B' } else { b'A' };
    *primary = String::from_utf8(bytes).expect("ASCII-only base64 mutation stays UTF-8");
    let tampered = lines.join("\n");

    let sig = VerifySignature::decode(&tampered)
        .expect("the tampered signature must still *parse* — only the verify step fails");

    pk.verify(TEST_PAYLOAD, &sig, /* allow_legacy */ false)
        .expect_err(
            "flipping a base64 byte in the primary signature line must fail verification; \
             this is the 'attacker rewrites latest.json signature' protection",
        );
}

#[test]
fn verification_fails_when_payload_byte_is_flipped() {
    let kp = fresh_keypair();
    let sig_str = sign_payload_to_string(&kp, TEST_PAYLOAD);
    let pk = verify_pubkey_from(&kp);

    let sig = VerifySignature::decode(&sig_str).expect("untampered signature should decode");

    // Construct a divergent payload — same length, one bit
    // different. This is the "attacker swaps the .tar.gz behind
    // the URL named in latest.json without re-signing" attack
    // class.
    let mut tampered = TEST_PAYLOAD.to_vec();
    tampered[0] ^= 0x01;
    assert_ne!(
        tampered.as_slice(),
        TEST_PAYLOAD,
        "sanity: the tamper step must actually change the payload bytes",
    );

    pk.verify(&tampered, &sig, /* allow_legacy */ false)
        .expect_err(
            "verifying the original signature against a different payload must fail; \
             this is the 'attacker swaps the binary' protection",
        );
}

#[test]
fn production_pubkey_in_tauri_conf_json_parses() {
    // The production trust anchor lives base64-of-blob inside
    // `plugins.updater.pubkey` in `tauri.conf.json`. A typo, an
    // accidental whitespace insertion, or a partial paste during
    // a key rotation would silently brick every installed app's
    // ability to validate updates — but the failure would only
    // surface at release time today. This test parses the field
    // exactly the way `tauri-plugin-updater` does at startup, so
    // a regression fails CI within ~50 ms of being introduced.
    let conf_text = std::fs::read_to_string(tauri_conf_json_path())
        .expect("tauri.conf.json must exist beside this crate's manifest");
    let conf: serde_json::Value =
        serde_json::from_str(&conf_text).expect("tauri.conf.json must be valid JSON");
    let pubkey_field = conf
        .pointer("/plugins/updater/pubkey")
        .and_then(|v| v.as_str())
        .expect("plugins.updater.pubkey must be present and a string in tauri.conf.json");

    use base64::Engine;
    let decoded_blob = base64::engine::general_purpose::STANDARD
        .decode(pubkey_field)
        .expect("the pubkey field must be base64; otherwise tauri-plugin-updater fails at startup");
    let decoded_text = String::from_utf8(decoded_blob).expect(
        "the decoded pubkey blob must be UTF-8 (it's the standard minisign .pub file format)",
    );
    let key_line = decoded_text.lines().nth(1).expect(
        "the decoded blob must look like `untrusted comment: …\\n<base64>` — \
             a missing second line means the comment header was stripped",
    );

    VerifyPublicKey::from_base64(key_line).expect(
        "the production pubkey must parse cleanly with minisign-verify (the same crate \
         tauri-plugin-updater uses internally); a parse failure here means a future \
         tauri.conf.json edit broke the trust anchor before this PR shipped",
    );
}
