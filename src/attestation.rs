use std::{fs, io::Write, path::{Path, PathBuf}};

use anyhow::{ensure, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

use crate::manifest::ArtifactManifest;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestAttestation {
    pub version: u32,
    pub algorithm: String,
    pub key_id: String,
    pub artifact_id: String,
    pub manifest_blake3: String,
    pub tool_version: String,
    pub signature: String,
}

fn key_bytes(path: &Path, expected_len: usize, kind: &str) -> anyhow::Result<Vec<u8>> {
    let text = fs::read_to_string(path).with_context(|| format!("failed to read {kind} key {}", path.display()))?;
    let bytes = STANDARD.decode(text.trim()).with_context(|| format!("{kind} key must be base64"))?;
    ensure!(bytes.len() == expected_len, "{kind} key must decode to {expected_len} bytes");
    Ok(bytes)
}

fn manifest_payload(manifest: &ArtifactManifest) -> anyhow::Result<Vec<u8>> {
    crate::manifest::validate_manifest_structure(manifest)?;
    Ok(serde_json::to_vec(manifest)?)
}

pub fn manifest_digest(manifest: &ArtifactManifest) -> anyhow::Result<String> {
    Ok(blake3::hash(&manifest_payload(manifest)?).to_hex().to_string())
}

pub fn default_attestation_path(store: &Path, artifact_id: &str) -> PathBuf {
    store.join("attestations").join(format!("{artifact_id}.ed25519.json"))
}

fn write_new_key(path: &Path, bytes: &[u8], kind: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)
        .with_context(|| format!("failed to create {kind} key {}; existing keys are never overwritten", path.display()))?;
    file.write_all(STANDARD.encode(bytes).as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

pub fn generate_key_pair(private_key_path: &Path, public_key_path: &Path) -> anyhow::Result<()> {
    ensure!(private_key_path != public_key_path, "private and public key paths must differ");
    let signing = SigningKey::generate(&mut OsRng);
    let verifying = VerifyingKey::from(&signing);
    write_new_key(private_key_path, &signing.to_bytes(), "private")?;
    if let Err(error) = write_new_key(public_key_path, &verifying.to_bytes(), "public") {
        return Err(error).context("private key was created but public key creation failed; preserve the private key and choose a new public-key path");
    }
    Ok(())
}

pub fn attest_manifest(manifest: &ArtifactManifest, private_key_path: &Path, key_id: &str) -> anyhow::Result<ManifestAttestation> {
    ensure!(!key_id.trim().is_empty() && key_id.len() <= 128, "--key-id must be between 1 and 128 bytes");
    let bytes = key_bytes(private_key_path, 32, "private")?;
    let secret: [u8; 32] = bytes.try_into().expect("length checked");
    let signing_key = SigningKey::from_bytes(&secret);
    let digest = manifest_digest(manifest)?;
    let signature = signing_key.sign(digest.as_bytes());
    Ok(ManifestAttestation {
        version: 1,
        algorithm: "ed25519".to_string(),
        key_id: key_id.trim().to_string(),
        artifact_id: manifest.artifact_id.clone(),
        manifest_blake3: digest,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        signature: STANDARD.encode(signature.to_bytes()),
    })
}

pub fn save_attestation(attestation: &ManifestAttestation, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    fs::write(path, serde_json::to_vec_pretty(attestation)?)?;
    Ok(())
}

pub fn verify_attestation(manifest: &ArtifactManifest, attestation_path: &Path, public_key_path: &Path) -> anyhow::Result<ManifestAttestation> {
    let attestation: ManifestAttestation = serde_json::from_slice(&fs::read(attestation_path)?)?;
    ensure!(attestation.version == 1, "unsupported attestation version {}", attestation.version);
    ensure!(attestation.algorithm == "ed25519", "unsupported attestation algorithm '{}'", attestation.algorithm);
    ensure!(attestation.artifact_id.eq_ignore_ascii_case(&manifest.artifact_id), "attestation artifact ID does not match manifest");
    ensure!(attestation.manifest_blake3 == manifest_digest(manifest)?, "attestation manifest digest does not match manifest");
    let bytes = key_bytes(public_key_path, 32, "public")?;
    let public: [u8; 32] = bytes.try_into().expect("length checked");
    let key = VerifyingKey::from_bytes(&public).context("invalid Ed25519 public key")?;
    let signature_bytes = STANDARD.decode(&attestation.signature).context("attestation signature must be base64")?;
    let signature = Signature::from_slice(&signature_bytes).context("invalid Ed25519 signature")?;
    key.verify(attestation.manifest_blake3.as_bytes(), &signature).context("attestation signature verification failed")?;
    Ok(attestation)
}
