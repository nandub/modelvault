#![cfg(feature = "signing")]

use std::fs;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{SigningKey, VerifyingKey};
use modelvault::{
    artifact::add_raw_artifact,
    attestation::{attest_manifest, save_attestation, verify_attestation},
    cas::LocalCas,
};
use tempfile::tempdir;

#[test]
fn ed25519_attestation_binds_the_complete_manifest_payload() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let source = temp.path().join("artifact.bin");
    fs::write(&source, b"signed artifact")?;
    let cas = LocalCas::open(temp.path().join(".modelvault"))?;
    let added = add_raw_artifact(&source, &cas, 4)?;
    let private = temp.path().join("private.key");
    let public = temp.path().join("public.key");
    let secret = [7u8; 32];
    let signing = SigningKey::from_bytes(&secret);
    let verifying = VerifyingKey::from(&signing);
    fs::write(&private, STANDARD.encode(secret))?;
    fs::write(&public, STANDARD.encode(verifying.to_bytes()))?;

    let attestation = attest_manifest(&added.manifest, &private, "release-test")?;
    let path = temp.path().join("artifact.ed25519.json");
    save_attestation(&attestation, &path)?;
    assert_eq!(verify_attestation(&added.manifest, &path, &public)?.key_id, "release-test");

    let mut changed = added.manifest.clone();
    changed.source_name = "changed-name.bin".to_string();
    assert!(verify_attestation(&changed, &path, &public).is_err());
    Ok(())
}
