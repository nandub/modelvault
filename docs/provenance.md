# ModelVault provenance

ModelVault records optional stable-source provenance for artifacts imported from a source that has a portable identity. The first provider supported is Hugging Face.

## Design rules

- Provenance does not participate in the BLAKE3 artifact ID. The artifact ID remains the hash of the artifact bytes.
- Provenance is optional and backward-compatible with existing manifest and pointer version 1 files.
- Machine-specific cache paths are not persisted as provenance.
- Hugging Face snapshot commit IDs are preferred as the resolved revision when the cached path exposes one.
- Derived artifacts do not copy a parent's external-source provenance. Their relationship to the parent is recorded as lineage, keeping “where this file was obtained” separate from “what it was derived from.”

## Hugging Face fields

- `provider`: `huggingface`
- `namespace`: repository owner/namespace when present
- `repository`: full Hugging Face repository ID
- `model_name`: repository basename
- `requested_revision`: user-requested revision, defaulting to `main`
- `resolved_revision`: immutable snapshot commit when available
- `filename`: imported artifact filename
- `source_uri`: stable ModelVault source identity such as `hf://org/model@commit/model.safetensors`

## Inspection

```powershell
modelvault provenance .\models\model.safetensors.mvptr
modelvault provenance .\models\model.safetensors.mvptr --json
```

## Attestations

With the optional `signing` Cargo feature, ModelVault can create and verify
Ed25519 attestations stored separately from manifests. An attestation signs the
BLAKE3 digest of ModelVault's deterministic serialized manifest payload, so it binds artifact
identity, chunk references, provenance, and lineage without changing any of
those identities. Private/public key files contain base64-encoded 32-byte raw
Ed25519 key material and must remain outside the repository.

```powershell
cargo run --features signing -- attest-keygen `
  --private-key C:\Keys\modelvault-release.private `
  --public-key C:\Keys\modelvault-release.public
cargo run --features signing -- attest .\models\model.safetensors.mvptr `
  --private-key C:\Keys\modelvault-release.private `
  --key-id release-2026-08
cargo run --features signing -- verify-attestation .\models\model.safetensors.mvptr `
  --public-key C:\Keys\modelvault-release.public
```
