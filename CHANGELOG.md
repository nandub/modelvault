# Changelog

## Unreleased

- Added a one-command Windows helper for checksum and Sigstore verification of
  published release archives.
- Bounded Safetensors header allocation during ingestion before parsing
  untrusted header data.
- Added opt-in phase-aware progress for large `add`, `add-raw`, and
  `materialize` operations.

## 1.7.6 - Release

- Added an optional signing-feature CI gate for build, tests, and strict Clippy.
- Updated optional signing key generation for `ed25519-dalek` 3 compatibility,
  continuing to use the operating system CSPRNG.

## 1.7.5 - Release workflow validation

- Fixed PowerShell parsing in the release version-consistency gate.

## 1.7.4 - Release automation
- Added a release-preparation helper and release workflow version-consistency gate.
- Added local and CI checks that parse the release scripts and exercise their
  core version and binary validations.
- Fixed release workflow script paths for Linux and macOS runners.
- Checked out release scripts in the publish job before invoking them.
- Install Cosign before the release verification script signs assets.

## 1.7.2 - Release smoke reliability

- Made the release smoke test retry release-asset downloads and report both
  hashes if a published checksum mismatch persists.
- Generate checksums and Sigstore bundles from archives downloaded from the
  GitHub Release, then smoke-test those uploaded release files.
- Fixed `SHA256SUMS` generation to write one record per release archive.

## 1.7.1 - Release pipeline hardening

- Added keyless Sigstore bundles for GitHub Release archives and `SHA256SUMS`.
- Pinned CI, security, and release workflow actions to immutable commits and
  the Semgrep container to a manifest digest.
- Fixed SHA-pinned Rust toolchain actions to receive their explicit toolchain
  inputs.
- Added weekly Dependabot updates for Cargo dependencies and GitHub Actions.
- Added a post-publish Linux archive checksum, extraction, and CLI smoke test.

## 1.7.0 - Remote lifecycle, derived artifacts, and CI

- Added `extract-tensors` for verified derived Safetensors output, with exact and prefix selectors, optional repository registration, and `extract-tensors` lineage.
- Added JSON and Markdown tensor-aware model comparison reports.
- Added optional Ed25519 manifest attestations and local key-pair generation behind the `signing` Cargo feature.
- Added `remote fsck`, `remote storage`, and dry-run/`--prune` `remote gc` for filesystem/UNC and optional S3/MinIO remotes.
- Added bounded, prefix-contained S3/MinIO audit listing and MinIO acceptance coverage for remote audit/storage/GC.
- Added opt-in Git post-checkout advice hooks that never fetch or materialize artifacts automatically.
- Added GitHub Actions CI for locked validation on Windows, Linux, and macOS, plus an optional S3 build gate.
- Added tag-triggered GitHub Release artifacts for Windows x86_64, Linux x86_64/ARM64, and macOS Intel/Apple Silicon, with binary sanity checks and SHA-256 checksums.
- Added scheduled RustSec dependency auditing and Semgrep static analysis with metrics disabled.
- Updated the optional AWS S3 SDK dependencies and documented temporary RustSec exceptions for remaining upstream HTTP/TLS advisories.
- No artifact-ID, CAS-ID, manifest-v1, pointer-v1, pack, delta, or repository-format identity changes.

## 1.6.1 - Rust 1.96 compatibility fix

- Removed two impossible `max_depth <= 256` checks from delta CLI paths where `max_depth` is already a `u8` (`0..=255`).
- Preserved the separate lineage `--max-depth <= 256` validation, which uses `usize` and remains meaningful.
- No lineage, manifest, pointer, CAS, pack, delta-format, provenance, or repository-format behavior changes.

## 1.6.0 - Phases 40-42: artifact lineage

- Added optional manifest/pointer lineage edges recording a parent artifact ID, derivation operation, and optional note.
- Added `modelvault derive <artifact> --parent <artifact> --operation <name> [--note <text>] [--stage]`.
- Added `modelvault lineage <artifact> [--json] [--max-depth <n>]` for recursive ancestry inspection.
- Lineage graph traversal preserves missing ancestors as unresolved nodes rather than silently dropping relationships.
- Rejects self-parent relationships and ancestry cycles before lineage metadata is committed.
- Caps lineage operation/note lengths, graph traversal work, and display depth.
- Legacy v1 manifests and pointers without lineage remain readable through serde defaults.
- Artifact IDs, CAS IDs, object representations, pack v2, delta format, repository version, and provenance semantics are unchanged.

## 1.5.1 - Hardening build fixes

- Fixed `repository.rs` to call the centralized `validate_manifest_structure()` function introduced in 1.5.0.
- Updated `scripts/Validate-ModelVault.ps1` to refresh `Cargo.lock` when it is missing **or** when its root `modelvault` package version does not match `Cargo.toml`, then enforce `--locked` validation.
- No storage-format, pointer, manifest, CAS, pack, delta, provenance, or remote protocol changes.

## 1.5.0 - Security hardening and documentation reconciliation

- Hardened `.mvptr` resolution so pointers can reference only `.modelvault/manifests/<artifact-id>.json`; traversal/arbitrary manifest paths are rejected.
- Cross-validates pointer artifact ID, logical size, format, source name, and pointer-carried provenance against the referenced manifest.
- Centralized manifest structural validation and applies it on load/save and before materialization.
- Materialization validates ranges before filesystem allocation and uses uniquely-created temporary files.
- Bounded Zstandard decompression for `MVZ1`, pack-v2 Zstd entries, and `MVD1` delta payloads.
- Hardened CAS temporary writes with `create_new` semantics instead of PID-only temporary filenames.
- Validates pack-index object IDs, byte ranges, raw sizes, and `pack_file` basenames so a malicious index cannot escape `.modelvault/packs`.
- Hugging Face cache symlinks are accepted only when their canonical targets remain inside the expected model cache directory.
- Import target containment is checked before directory creation, preventing rejected targets from leaving directories outside the repository.
- Added `push/pull --deep-verify` for body-level BLAKE3 verification of existing destination objects; normal S3 verification may continue to use ModelVault metadata for speed.
- Added warnings for non-local custom S3/MinIO endpoints configured with plaintext HTTP.
- Added `SECURITY.md`, updated storage-format documentation for pack v2/MVZ1/MVD1, rewrote the README around current behavior, and repaired changelog drift.
- Added security regression tests for pointer traversal/identity mismatch, pre-allocation manifest validation, bounded decompression, and import target side effects.
- Added `scripts/Validate-ModelVault.ps1` to generate a missing `Cargo.lock` on a Rust-enabled workstation and then enforce `--locked` build/test/Clippy validation, with optional S3/audit/deny checks.
- No artifact-ID, CAS-ID, manifest-v1, pointer-v1, pack-v2, delta, or repository-format identity changes.

## 1.4.1

- Fixed Rust ownership error in Hugging Face provenance construction by making the source revision an owned string before moving `resolved_revision` into the provenance record.
- No pointer, manifest schema, CAS, pack, delta, or repository-format changes.

## 1.4.0 - Phases 37-39

### Added
- Optional provenance metadata on artifact manifests and pointer files while keeping legacy v1 files readable.
- Hugging Face provenance capture: provider, namespace, repository, model name, requested revision, resolved snapshot commit, filename, and stable `hf://` source URI.
- `modelvault provenance <manifest-or-pointer>` with human-readable and `--json` output.
- Provenance regression tests, including legacy manifest compatibility and protection against leaking local Hugging Face cache paths.

### Compatibility
- Artifact IDs, chunk IDs, pack formats, delta formats, and repository format are unchanged.
- Existing manifests and `.mvptr` files without provenance continue to deserialize normally.

## 1.3.6

- Fixed strict Clippy `cloned_ref_to_slice_refs` warning in Git manifest staging.
- Replaced the temporary cloned `PathBuf` slice with `std::slice::from_ref`.
- No behavioral, manifest, pointer, CAS, pack, delta, or repository format changes.

## 1.3.5

- Fixed `--stage` when older repositories still contain a broad `.modelvault/` ignore rule.
- Automatically migrates exact legacy `.modelvault` root ignore rules to storage-only ignore rules.
- Force-stages ModelVault manifest metadata so global/user ignore rules cannot hide required manifests.
- Updated the shipped `.gitignore` template so manifests are trackable by default.

## 1.3.4

- Fix Windows startup stack overflow affecting all CLI invocations, including `--version` and `help`, by running Clap parsing/dispatch on a dedicated 8 MiB stack.
- No command syntax, pointer, manifest, CAS, pack, delta, or repository-format changes.

## 1.3.3

- Fixed Windows Hugging Face cache resolution stack overflow by avoiding recursive link-following probes for snapshot files.
- Cache resolution now treats regular files and snapshot symlinks as valid candidates using `symlink_metadata`.

## 1.3.2

- Refactored the Hugging Face import command to use an `ImportHfOptions` options struct instead of an eight-argument helper function.
- Resolves strict Clippy `too_many_arguments` without suppressing the lint.
- No behavioral, manifest, pointer, CAS, pack, delta, or repository format changes.

## 1.3.1

- Fixed Windows import-target path normalization so repository-local targets no longer expose the `\\?\` verbatim-path prefix.
- Canonical paths are now used only for repository-containment validation; user-facing target and pointer paths preserve the normal repository path form.
- No manifest, pointer, CAS, pack, delta, or repository format changes.

## 1.3.0 - Import workflows

- Added `modelvault import <source> --to <repo-path>` for ingesting external artifacts directly into CAS while creating a repository-local logical path and `.mvptr` file.
- Added `modelvault import-hf <repo-id>` with Hugging Face cache resolution, revision/filename/cache overrides, `--local-only`, and fallback to the official `hf download` CLI.
- `track` now requires the artifact to already be inside the Git work tree; external files receive a clear instruction to use `import`.
- Imported logical artifact paths are ignored in Git while external source files remain untouched.
- Added regression coverage for repository-local import targets and Hugging Face cache/ref resolution.
- Pointer, manifest, CAS, pack-v2, and repository formats remain unchanged.

## 1.2.1

- Fixed `track --stage` for artifacts located outside the Git work tree.
- External artifacts now default to repository-local pointers under `models/external/<artifact-prefix>-<source>.mvptr`.
- Added `track --pointer <path>` to explicitly select the pointer location; relative paths resolve from the Git root.
- External source paths are no longer written into `.gitignore`.
- Physical ModelVault directories (`objects`, `tmp`, `packs`, `deltas`) are ignored consistently.
- Existing in-repository tracking behavior remains unchanged.

# 1.2.0

- Added benchmark snapshot comparison.
- Added chunk-level diagnostics for one or two artifacts.
- Added read-only policy simulation across chunk sizes and delta thresholds.
- No storage format changes.

## 1.1.0

- Phase 31: decomposed storage efficiency into deduplication, compression, delta, duplicate-representation, metadata, and net physical effects.
- Phase 32: added order-independent per-artifact exclusive/shared logical bytes and approximate physical attribution.
- Phase 33: added `benchmark-repo` snapshots with JSON output for repeatable release/strategy comparisons.
- Renamed the ambiguous storage output metric `Logical savings` to `Dedup savings` and added compression/delta/net metrics.


## 1.0.2

- Fix strict Clippy `possible_missing_else` warning in `optimize_cmd` by formatting adjacent optional pack/index output checks as separate statements.
- No storage-format, manifest, pointer, CAS-ID, or repository-format changes.

## 1.0.1

- Clean up strict Clippy findings in the pack-v2 optimizer.
- Derive `Default` for `PackEncoding` and mark `Raw` as the default variant.
- Reformat old-pack cleanup so control flow is explicit and passes `clippy::possible_missing_else`.
- No storage-format, manifest, pointer, CAS identity, or repository compatibility changes.

## 1.0.0

- Phase 28: physical storage accounting distinguishes true orphan bytes from duplicate representations and reports loose/raw, loose/Zstd, delta, pack data/index, manifest, and metadata bytes.
- Phase 29: pack format v2 adds per-object raw/Zstd encoding while remaining backward-readable with v1 indexes.
- Phase 30: `modelvault optimize [--dry-run]` selects compressed pack entries versus smaller persistent deltas, cleans redundant physical copies, and verifies all logical objects after cleanup.

## 0.9.1

- Fixed `policy_skips_delta_when_savings_are_insufficient` test fixture.
- Replaced highly periodic modulo-251 byte patterns with deterministic unrelated xorshift streams so full and XOR-delta payloads are both effectively incompressible.
- The test now reliably verifies that a strict 95% minimum-savings policy rejects an unhelpful persistent delta.
- No production storage format, manifest, pointer, CAS identity, or delta policy semantics changed.

## 0.8.1

- Fixed the Phase 24 analytics regression test fixture: two byte-identical files intentionally resolve to the same content-addressed artifact manifest, so the cross-artifact reuse test now uses two distinct artifacts that share seven of eight fixed-size chunks.
- Added a regression test documenting that identical content maps to one logical artifact ID/manifest even when ingested from different source paths.

## 0.8.0

- Phase 22: added experimental `delta-analyze` to measure aligned XOR+Zstd delta potential without changing manifest semantics.
- Phase 23: added `pack-verify` and `pack-compact` for full pack integrity validation and consolidation of existing loose/packed objects.
- Phase 24: added repository `analytics` with per-artifact reuse, global deduplication savings, and physical/logical storage ratios.
- Removed the stale unused `HashMap` import from `src/cas/local.rs`.
- Pointer, manifest, object ID, repository v1, and remote formats remain compatible with 0.7.x.

## 0.7.1

- Fixed deep `fsck` on compressed CAS objects.
- `verify_artifact` now compares manifest chunk size against decoded logical bytes rather than physical loose-file size.
- The same verification path now works for raw loose, Zstandard-compressed loose, and packed objects.
- Added a regression test covering deep fsck after Zstandard migration.


## 0.7.0

### Phase 19 — Repository format/version safeguards

- Added `.modelvault/repository.json` with explicit repository format version, object hash algorithm, loose-object compression policy, and pack format version.
- Existing repositories without metadata are upgraded non-destructively on first open with the legacy-compatible `compression: none` policy.
- Added `modelvault repo-info`.
- Repository open now rejects unsupported repository/object-hash/pack format versions instead of silently guessing.

### Phase 20 — Transparent CAS compression

- Added optional Zstandard physical encoding for loose CAS objects.
- BLAKE3 object IDs remain hashes of the original logical bytes; compression never changes manifests or `.mvptr` files.
- Added `modelvault migrate --compression zstd|none` for verified in-place loose-object migration.
- Mixed raw/compressed repositories remain readable because each compressed object carries a small `MVZ1` envelope.
- Default Cargo feature `compression` enables Zstandard; it can be disabled with `--no-default-features` for a raw-only build.
- Windows-safe temp/backup replacement is used when rewriting existing files.

### Phase 21 — Pack/index storage

- Added immutable `.mvpack` files plus JSON indexes under `.modelvault/packs/`.
- Added `modelvault repack` and the explicit `--prune-loose` reclamation switch.
- CAS lookup transparently falls back from loose objects to pack indexes.
- Packed objects are verified before any loose source object is deleted.
- Existing manifests, pointer files, Git history, filesystem remotes, and S3 object IDs are unchanged.

### Tests

- Repository metadata persistence.
- Zstandard migration in both directions with object identity preservation.
- Pack read/verify after loose objects are removed.
- Full artifact materialization from packed-only CAS content.

## 0.6.1

- Made AWS S3/MinIO support optional behind the `s3` Cargo feature to avoid forcing the large AWS SDK dependency graph into normal local/NAS builds and tests.
- Disabled dev/test debug symbols to reduce Windows compiler/linker virtual-memory pressure.

## 0.6.0

- Added S3-compatible object storage, MinIO convenience configuration, retries, and transfer telemetry.

## 0.9.0 - Phases 25-27

### Added
- Persistent `MVD1` XOR+Zstd delta objects keyed by the target object's ordinary BLAKE3 identity.
- Recursive delta reconstruction with cycle detection and final target BLAKE3 verification.
- Repository-level delta policy (`delta_min_savings_pct`, default 20%; `max_delta_depth`, default 2) with backward-compatible metadata defaults.
- `modelvault delta-policy` to configure automatic full-vs-delta selection policy.
- `modelvault delta-optimize` to convert eligible aligned changed loose objects to bounded persistent deltas.
- Delta dependency reachability for `fsck`, storage reporting, and garbage collection.
- Tests for delta round-trip, chain limits, policy rejection, legacy repository metadata, and GC base preservation.

### Compatibility
- Manifest format remains version 1 and unchanged.
- `.mvptr` format remains unchanged.
- Object IDs remain BLAKE3 hashes of fully reconstructed logical bytes.
- Filesystem and S3 remotes continue transferring full logical CAS objects; remote format does not need to understand deltas.
