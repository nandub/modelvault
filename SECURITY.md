# ModelVault Security Policy

## Supported versions

Security fixes are developed against the latest ModelVault release. Older experimental releases should be upgraded before reporting behavior that is already fixed in the current release.

## Reporting a vulnerability

Do not publish exploit details in a public issue before maintainers have had an opportunity to investigate. Include the ModelVault version, operating system, reproduction steps, affected command, and whether the repository, pointer, manifest, cache, or remote was attacker-controlled.

## Trust model

ModelVault treats these inputs as **untrusted metadata or content** and validates them before use:

- `.mvptr` files obtained from Git;
- artifact manifests obtained from Git or remotes;
- CAS object bytes and compressed/delta/pack representations;
- filesystem and object-store remote content;
- Hugging Face cache entries and snapshot links.

ModelVault assumes the local operating-system account, the ModelVault executable, and the Rust/toolchain installation are trusted.

### Git pointers and manifests

A pointer may reference only its content-addressed manifest:

```text
.modelvault/manifests/<artifact-id>.json
```

ModelVault rejects arbitrary, absolute, or traversal-based manifest references. Pointer artifact identity, logical size, format, source name, and any pointer-carried provenance must agree with the referenced manifest.

Manifest structure is validated before materialization or object traversal. Chunk ranges must be contiguous and non-overlapping, arithmetic must not overflow, object IDs must be valid BLAKE3 identifiers, and tensor ranges must stay inside the artifact.

### Content integrity

Artifact and CAS identities are BLAKE3 hashes of logical/original bytes. Physical storage may be raw, Zstandard-compressed, packed, or delta-based without changing logical identity.

Materialization verifies the reconstructed complete artifact hash before publishing the output file. Remote transfer verifies source bytes before accepting them as a ModelVault object.

### Compressed data

Compressed loose objects, compressed pack entries, and delta payloads are decoded with an output bound derived from their declared logical size. Data that expands beyond that bound is rejected.

### Temporary files

Materialization and CAS writes use uniquely-created temporary files with `create_new` semantics to avoid overwriting attacker-precreated temporary paths. The final destination is published only after verification succeeds.

### Hugging Face cache

Snapshot files may be symlinks as part of normal Hugging Face cache behavior. ModelVault resolves those links for containment validation and accepts them only when the canonical target remains inside the expected model cache directory. Provenance records stable Hugging Face identity and revision data, not the local cache path.

### Remotes

Filesystem remotes are trusted only as storage locations; their object content is still hash-verified when transferred.

S3-compatible backends support two verification modes:

- **fast/backend verification** may use ModelVault BLAKE3 object metadata to avoid downloading an existing object;
- `push/pull --deep-verify` reads object bodies and validates BLAKE3 content before reuse.

Use HTTPS for non-local S3-compatible endpoints. ModelVault warns when a non-loopback custom endpoint uses plaintext HTTP. Plain HTTP is intended only for local development such as a local MinIO instance.

### Credentials

ModelVault configuration stores remote locations and optional AWS profile names, not AWS secret keys. AWS credentials are resolved through the AWS SDK credential chain when the optional `s3` feature is enabled.

## Dependency and build security

ModelVault is an executable application and release builds should include `Cargo.lock` and use locked dependency resolution:

```powershell
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Recommended supply-chain checks:

```powershell
cargo audit
cargo deny check
```

The normal build excludes the AWS SDK. S3 support is opt-in with `--features s3`.

## Non-goals

ModelVault does not provide malware scanning, model safety evaluation, encryption-at-rest, access control, Git signing, remote server authentication beyond the selected backend, or protection from a fully compromised local account/host.


## Lineage metadata

Lineage edges stored in Git-controlled manifests/pointers are untrusted metadata. ModelVault validates parent artifact IDs and metadata bounds, rejects self references, prevents known ancestry cycles when adding edges, caps traversal depth/work, and does not use lineage strings as filesystem paths or shell commands. A missing parent manifest is reported as unresolved rather than implicitly trusted or fetched.
