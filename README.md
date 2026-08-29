# ModelVault

ModelVault is an experimental tensor-aware, content-addressed storage layer for large AI artifacts. Git tracks small pointer/manifests while ModelVault stores reusable binary chunks in a local or remote CAS.

Current release line: **1.6.x**. The implementation includes Safetensors-aware chunking and selective derived extraction, exact materialization, Git pointers, imports, Hugging Face provenance and lineage, JSON/Markdown model comparison reports, optional Ed25519 attestations, repository integrity/GC, pack/delta optimization, filesystem remotes, optional S3/MinIO audit/lifecycle support, and opt-in checkout advice.

## Core invariants

- Artifact identity = BLAKE3 of complete original bytes.
- CAS identity = BLAKE3 of decoded logical chunk bytes.
- Physical representation never changes logical identity.
- Materialization must reproduce the exact original artifact hash.
- Git-controlled pointers/manifests are treated as untrusted metadata and validated before filesystem use.
- Safetensors tensor-aware chunks never cross tensor boundaries.

See [SECURITY.md](SECURITY.md) for the trust model and [docs/storage-format.md](docs/storage-format.md) for physical formats.

## Build and validation

Rust 1.94.1 or newer is required.

```powershell
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

Release/source-control builds should commit `Cargo.lock` and use:

```powershell
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

A Windows validation helper is included. It generates `Cargo.lock` if a clean source tree does not yet contain one, then runs locked build/test/Clippy checks:

```powershell
.\scripts\Validate-ModelVault.ps1
.\scripts\Validate-ModelVault.ps1 -WithS3 -SecurityTools
.\scripts\Validate-ModelVault.ps1 -WithS3 -WithMinio
```

`-WithMinio` requires Docker Desktop and runs a disposable loopback-only MinIO
push/pull/checkout acceptance test. See [the remote documentation](docs/remotes.md#disposable-minio-acceptance-test).

Optional security tooling:

```powershell
cargo audit
cargo deny check
```

The default build excludes the heavyweight AWS SDK. Enable S3/MinIO explicitly:

```powershell
cargo build --features s3
cargo test --features s3
```

On memory-constrained Windows systems:

```powershell
$env:CARGO_BUILD_JOBS = '1'
cargo build --features s3
Remove-Item Env:CARGO_BUILD_JOBS
```

## Initialize

```powershell
modelvault init
```

The repository uses:

```text
.modelvault/
├── manifests/       # Git-trackable reconstruction metadata
├── objects/         # loose CAS objects; ignored by Git
├── packs/           # packed physical CAS; ignored by Git
├── deltas/          # persistent delta representations; ignored by Git
├── repository.json  # physical-format/policy metadata
└── config.json      # named remotes when configured
```

Some storage directories are created lazily.

## Inspect and add

```powershell
modelvault inspect .\models\model.safetensors
modelvault inspect .\models\model.safetensors --json
modelvault add .\models\model.safetensors
modelvault add .\artifact.bin --format raw --chunk-size 1048576
```

Safetensors is auto-detected by extension and uses tensor-bounded chunking. Unknown formats can use raw chunking.

## Track an artifact already inside Git

`track` is for an artifact whose logical path is already inside the Git work tree:

```powershell
modelvault track .\models\model.safetensors --stage
```

This creates `model.safetensors.mvptr`, writes its manifest under `.modelvault/manifests/`, ignores the large logical artifact and physical CAS storage, and optionally stages Git metadata.

## Import an external artifact

`import` ingests an external file without first copying the large source into the repository. `--to` defines the logical path that checkout will materialize:

```powershell
modelvault import C:\external\model.safetensors `
  --to models\demo\model.safetensors `
  --stage
```

## Import from Hugging Face

ModelVault first resolves the normal Hugging Face cache and, unless `--local-only` is supplied, can invoke the official `hf download` CLI on a cache miss:

```powershell
modelvault import-hf sentence-transformers/all-MiniLM-L6-v2 --stage
```

Useful options:

```powershell
modelvault import-hf org/model `
  --filename model.safetensors `
  --revision main `
  --to models\org-model\model.safetensors `
  --cache-dir D:\hf-cache\hub `
  --stage
```

Provenance records stable provider/repository/revision identity and deliberately excludes the machine-specific cache path.

```powershell
modelvault provenance .\models\all-MiniLM-L6-v2\model.safetensors.mvptr
modelvault provenance .\models\all-MiniLM-L6-v2\model.safetensors.mvptr --json
```

### Record and inspect model lineage

Lineage records relationships between immutable artifact identities without changing CAS/object IDs. The child manifest stores the parent artifact ID plus an operation label and optional note.

```powershell
modelvault derive .\models\my-tuned-model\model.safetensors.mvptr `
    --parent .\models\all-MiniLM-L6-v2\model.safetensors.mvptr `
    --operation fine-tune `
    --note "domain adaptation run 42" `
    --stage

modelvault lineage .\models\my-tuned-model\model.safetensors.mvptr
modelvault lineage .\models\my-tuned-model\model.safetensors.mvptr --json
```

Lineage is optional and backward-compatible. Missing ancestor manifests are displayed as unresolved nodes, and ModelVault rejects self-parent or cyclic derivation relationships.

## Checkout / materialize

From a Git pointer:

```powershell
modelvault checkout .\models\model.safetensors.mvptr
```

To create a derived Safetensors file containing only named tensors, use
`extract-tensors`. This does not alter the source artifact or its identity:

```powershell
modelvault extract-tensors .\models\model.safetensors.mvptr `
  --tensor encoder.layer.0.attention.self.query.weight `
  --tensor encoder.layer.0.attention.self.query.bias `
  --output .\exports\layer-0-query.safetensors
```

For a complete module or layer, repeat `--prefix`; a prefix selects all tensor
names that start with it. Exact `--tensor` and `--prefix` selectors can be
combined. Empty or unmatched prefixes are rejected, so a typo cannot silently
produce an empty derived artifact.

```powershell
modelvault extract-tensors .\models\model.safetensors.mvptr `
  --prefix encoder.layer.0. `
  --output .\exports\layer-0.safetensors
```

The source artifact is fully hash-verified before ModelVault publishes the
derived output. The command reports the derived file's BLAKE3 ID.

To register that derived file in the repository and preserve its relationship
to the source, add `--to`. ModelVault imports the resulting Safetensors file,
writes its pointer and manifest, and records an `extract-tensors` lineage edge.
Use `--stage` to stage the pointer and manifest.

```powershell
modelvault extract-tensors .\models\model.safetensors.mvptr `
  --tensor encoder.layer.0.attention.self.query.weight `
  --output .\exports\query.safetensors `
  --to .\models\derived\query.safetensors `
  --stage
```

From a manifest directly:

```powershell
modelvault materialize `
  .\.modelvault\manifests\<artifact-id>.json `
  .\restored-model.safetensors
```

Pointer paths are restricted to `.modelvault/manifests/<artifact-id>.json`; pointer and manifest identity fields must agree. Manifest structure is validated before output allocation. The final reconstructed artifact is published only if its BLAKE3 hash matches `artifact_id`.

## Verify and repository integrity

```powershell
modelvault verify .\.modelvault\manifests\<artifact-id>.json
modelvault fsck
modelvault fsck --deep
modelvault storage
modelvault analytics --detailed
```

`fsck` validates manifest structure and object presence. `--deep` additionally validates logical object hashes.

## Model comparison reports

`diff` compares Safetensors metadata and chunk reuse. Use `--json` for
automation or `--markdown` to write a review-ready report with artifact IDs,
provenance/lineage summaries, tensor changes, and byte/reuse totals.

```powershell
modelvault diff .\models\base.safetensors.mvptr .\models\candidate.safetensors.mvptr --json
modelvault diff .\models\base.safetensors.mvptr .\models\candidate.safetensors.mvptr `
  --markdown .\reports\model-comparison.md
```

## Diff and diagnostics

```powershell
modelvault diff .\base.safetensors.mvptr .\finetune.safetensors.mvptr
modelvault diff .\base.safetensors.mvptr .\finetune.safetensors.mvptr --all
modelvault benchmark .\base.safetensors .\finetune.safetensors
modelvault chunk-stats .\base.mvptr .\finetune.mvptr
modelvault simulate-policy .\base.mvptr .\finetune.mvptr
modelvault benchmark-repo --output .\benchmark.json
modelvault benchmark-compare .\before.json .\after.json
```

## Physical storage management

Repository policy is visible with:

```powershell
modelvault repo-info
```

Physical maintenance commands include:

```powershell
modelvault migrate --compression zstd --level 3
modelvault repack --prune-loose
modelvault pack-verify
modelvault pack-compact --prune-old --prune-loose
modelvault delta-analyze .\base.mvptr .\finetune.mvptr
modelvault delta-optimize .\base.mvptr .\finetune.mvptr
modelvault delta-policy --min-savings-pct 20 --max-depth 2
modelvault optimize
modelvault optimize --dry-run
modelvault gc
modelvault gc --prune
```

Pack v2 chooses `raw` or `zstd` per object. Persistent deltas are retained only when policy says they are smaller enough. Compressed reads are bounded by declared logical sizes.

## Remotes

Filesystem/UNC remote:

```powershell
modelvault remote add origin D:\ModelVaultRemote --default
modelvault push .\models\model.safetensors.mvptr --remote-name origin
modelvault pull .\models\model.safetensors.mvptr --remote-name origin
modelvault remote fsck origin --deep
modelvault remote storage origin
modelvault remote gc origin
modelvault remote gc origin --prune
```

Direct filesystem path remains supported:

```powershell
modelvault push .\models\model.safetensors.mvptr --remote D:\ModelVaultRemote
```

S3/MinIO requires the `s3` feature. Credentials are resolved by the AWS SDK; ModelVault config stores no secret keys.

For stronger assurance when deciding whether existing remote objects can be reused:

```powershell
modelvault push .\models\model.safetensors.mvptr --remote-name origin --deep-verify
modelvault pull .\models\model.safetensors.mvptr --remote-name origin --deep-verify
```

Fast S3 verification may trust ModelVault BLAKE3 metadata from `HEAD`; `--deep-verify` downloads and hashes object bodies. Use HTTPS for non-local custom S3-compatible endpoints. Local HTTP is appropriate for development MinIO; non-local plaintext HTTP produces a warning.

## Optional Git checkout advice

Install an opt-in post-checkout hook to detect ModelVault pointers after a
clone, branch checkout, or reset and print explicit recovery commands:

```powershell
modelvault git-hook install
```

The hook only runs `modelvault checkout-advice`. It never contacts a remote,
pulls objects, or materializes files automatically. Existing Git hooks are
preserved unless `--force` is explicitly supplied.

## Optional release attestations

Optional Ed25519 attestations bind the deterministic ModelVault manifest
serialization without changing artifact identity. See [Provenance and
attestations](docs/provenance.md#attestations) for the feature-gated key,
attest, and verification workflow.

See [docs/remotes.md](docs/remotes.md).

## Documentation

- [Storage format](docs/storage-format.md)
- [Remotes](docs/remotes.md)
- [Measurement and analytics](docs/measurement.md)
- [Provenance](docs/provenance.md)
- [Lineage](docs/lineage.md)
- [Security policy and trust model](SECURITY.md)
- [1.5.0 security review](docs/security-review-1.5.0.md)
- [Release history](CHANGELOG.md)

## Git ignore policy

Physical data is ignored, but manifests are intentionally trackable:

```text
.modelvault/objects/
.modelvault/tmp/
.modelvault/packs/
.modelvault/deltas/
```

The logical model files themselves are ignored when tracked/imported; their `.mvptr` files and manifests are what Git records.

## License

Apache-2.0 OR MIT.
