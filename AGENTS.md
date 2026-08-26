# AGENTS.md

## Purpose

This file defines the working rules for coding agents contributing to **ModelVault**.
It is intended to be usable by Codex, Claude Code, GitHub Copilot agents, and similar autonomous or semi-autonomous coding tools.

ModelVault is a Git-compatible storage and source-control layer for large machine-learning artifacts. It keeps Git responsible for source history while ModelVault manages large model data in a content-addressed store (CAS), using lightweight pointers and manifests committed to Git.

The current project baseline is **ModelVault 1.6.1**.

---

## Core Project Goals

ModelVault should:

1. Keep Git usable for ML projects without storing large model binaries directly in Git history.
2. Treat an ML artifact as a logical artifact, not merely as an opaque file.
3. Preserve exact byte-for-byte artifact reconstruction.
4. Keep logical object identity independent from physical storage representation.
5. Support model-aware storage techniques such as tensor-bounded chunking, deduplication, compression, packs, and deltas.
6. Support portable provenance and model lineage without changing artifact identity.
7. Work well on Windows as a first-class development environment.
8. Preserve repository compatibility across releases whenever practical.
9. Prefer explicit integrity validation over implicit trust.
10. Avoid introducing unnecessary runtime or dependency weight into the default build.

---

## Non-Negotiable Architectural Invariants

Agents MUST preserve the following invariants unless the task explicitly requires a repository-format migration and the migration is documented and tested.

### Artifact identity

An artifact ID is:

```text
BLAKE3(exact reconstructed logical artifact bytes)
```

Artifact IDs MUST NOT depend on:

- compression
- pack layout
- delta representation
- source path
- repository path
- provenance metadata
- lineage metadata
- timestamps
- machine-specific state

### CAS object identity

A CAS object ID is:

```text
BLAKE3(logical/original object bytes)
```

Physical representations MAY include:

- raw loose objects
- MVZ1 compressed loose objects
- pack-v1 entries
- pack-v2 raw entries
- pack-v2 Zstd entries
- MVD1 delta objects

The physical encoding MUST NOT change the object ID.

### Exact reconstruction

Materialization must reconstruct the original artifact byte-for-byte.

After reconstruction, ModelVault must verify the artifact against its expected BLAKE3 artifact ID before publishing the final output.

### Git boundary

Git should track lightweight, reviewable metadata such as:

- `*.mvptr`
- `.modelvault/manifests/*.json`
- `.modelvault/repository.json`
- `.modelvault/config.json` when appropriate
- documentation
- source code
- `Cargo.lock`

Git should NOT normally track physical model storage such as:

- `.modelvault/objects/`
- `.modelvault/packs/`
- `.modelvault/deltas/`
- `.modelvault/tmp/`
- large materialized model binaries managed by ModelVault

### Pointer trust boundary

Treat `.mvptr` files as untrusted Git-controlled metadata.

A pointer manifest path must resolve only to:

```text
.modelvault/manifests/<artifact-id>.json
```

Do not introduce generic pointer-controlled filesystem reads.

When resolving a pointer, validate at minimum:

- pointer artifact ID
- pointer logical size
- pointer artifact format
- manifest identity
- manifest path containment
- provenance consistency when both pointer and manifest contain provenance

### Manifest trust boundary

Treat manifests as untrusted metadata until validated.

Before materialization or filesystem allocation, validate:

- supported manifest version
- artifact ID syntax
- object IDs
- chunk sizes
- checked offset arithmetic
- contiguous chunk ranges
- no gaps
- no overlaps
- total logical size
- tensor ranges when applicable

Do not allocate or resize the output artifact based only on unvalidated manifest values.

---

## Repository Layout

The repository commonly contains:

```text
.modelvault/
├── manifests/
├── objects/
├── packs/
├── deltas/
├── repository.json
└── config.json

models/
└── <model-name>/
    └── model.safetensors.mvptr

src/
tests/
docs/
scripts/
Cargo.toml
Cargo.lock
README.md
CHANGELOG.md
SECURITY.md
AGENTS.md
```

Some directories may be created lazily.

---

## Important Source Areas

The exact file set may evolve, but agents should expect responsibilities similar to the following:

- `src/main.rs`
  - CLI definitions and command dispatch
- `src/manifest.rs`
  - artifact manifest definitions and validation
- `src/import.rs`
  - generic import and Hugging Face import helpers
- `src/repository.rs`
  - repository-wide integrity and maintenance logic
- `src/cas/`
  - local CAS representations, pack handling, compression, deltas
- `src/remote/` or equivalent
  - filesystem and optional S3-compatible object stores
- `src/pointer.rs` or equivalent
  - `.mvptr` parsing, writing, and validated resolution
- `src/lineage.rs` or equivalent
  - derivation metadata and lineage graph traversal
- `tests/`
  - integration and regression tests
- `docs/`
  - authoritative technical documentation

Before editing, inspect the current tree instead of assuming filenames from this document are exact.

---

## Current User-Facing Concepts

### `track`

Use when an artifact already exists inside the Git working tree.

External files should not be silently treated as tracked repository files.

### `import`

Use for an external local artifact.

Example concept:

```text
external file
    -> ingest into ModelVault CAS
    -> create repository-local logical target
    -> create .mvptr
    -> create manifest
```

The source file does not need to be copied into the repository.

### `import-hf`

Use for Hugging Face repositories.

The command should:

1. Prefer an existing Hugging Face cache entry.
2. Resolve the requested revision to an exact snapshot when possible.
3. Avoid permanently storing machine-specific cache paths in provenance.
4. Optionally use the official `hf` CLI when a cache download is needed.
5. Respect `--local-only`.

### Provenance

Provenance answers:

```text
Where did this artifact originate?
```

For Hugging Face, stable metadata may include:

- provider
- namespace
- repository
- model name
- filename
- requested revision
- resolved revision
- stable `hf://...` source URI

Do not persist local cache paths as stable provenance.

### Lineage

Lineage answers:

```text
What artifact(s) was this artifact derived from?
```

A derivation edge may contain:

- parent artifact ID
- operation label
- optional note

Operation labels are intentionally extensible, for example:

- `fine-tune`
- `quantize`
- `convert`
- `merge`
- `distill`
- `prune`
- `adapter-merge`
- `continued-pretraining`

Lineage metadata MUST NOT change artifact identity.

Lineage traversal must:

- reject known cycles when adding edges
- detect cycles when reading/displaying graphs
- tolerate missing ancestor manifests
- enforce traversal/resource limits

---

## Storage Model

### Loose objects

Loose objects may be stored raw or using MVZ1 compression according to repository policy.

### Pack format

Pack format v2 supports per-entry encoding such as:

- raw
- Zstd

Pack indexes must be treated as untrusted metadata.

Validate:

- pack filename containment
- object IDs
- offsets
- stored sizes
- logical sizes
- checked range arithmetic
- entries do not extend beyond pack length

Do not allow an index to point outside `.modelvault/packs/`.

### Delta representation

MVD1 deltas must preserve the target logical object ID.

After reconstructing a delta target, verify:

```text
BLAKE3(reconstructed bytes) == target object ID
```

Delta chains must remain bounded.

Current repository defaults historically include:

```text
Delta minimum savings: 20%
Maximum delta depth:    2
```

Do not assume defaults if repository metadata provides explicit values.

### Compression safety

Do not use unbounded decompression for untrusted stored objects.

Zstd decoding should stop once decoded output exceeds the expected logical size.

Apply this to:

- MVZ1
- pack-v2 compressed entries
- MVD1 payloads

---

## Remote Storage Rules

ModelVault supports filesystem-style remotes and optional S3-compatible remotes.

### Logical transfer invariant

Remote transfer should deal in full logical object contents unless a remote format explicitly defines otherwise.

A remote should not need to understand the sender's local pack or delta representation.

### Verification

Fast verification may use trusted metadata where supported.

Deep verification should verify object bodies with BLAKE3.

Where available, preserve the distinction between:

```text
fast verification
```

and:

```text
--deep-verify
```

### S3

S3 support is optional and must not become a mandatory dependency of the default build.

Reason: the AWS Rust SDK has historically imposed significant compile-time and memory overhead on Windows.

Keep S3 feature-gated unless there is an explicit architectural decision to change that.

For custom S3/MinIO endpoints:

- loopback HTTP is acceptable for local development
- prefer HTTPS for non-local endpoints
- warn on non-loopback plaintext HTTP

Do not persist AWS secrets in ModelVault repository configuration.
Use the AWS SDK credential chain/profile mechanisms.

---

## Temporary Files and Filesystem Safety

Temporary output names must not rely only on the process ID.

Prefer:

- unique unpredictable names
- `create_new(true)` semantics
- secure temporary-file APIs

Before creating directories from user-provided logical paths:

1. perform lexical validation
2. reject traversal
3. validate the nearest existing ancestor
4. verify repository containment
5. only then create directories

Avoid filesystem mutation before validation has succeeded.

Be careful with Windows path behavior:

- canonical paths may use the `\\?\` prefix
- path separators may differ
- canonicalization may resolve symlinks/junctions
- user-facing paths should avoid leaking unnecessary verbatim-path prefixes

Do not compare Windows paths as naïve raw strings when semantic `Path` comparison is appropriate.

---

## Hugging Face Cache Safety

Hugging Face snapshot entries may be symlinks or link-like filesystem objects on Windows.

Support valid snapshot links, but treat the cache as a trust boundary.

When resolving symlinks, ensure the canonical target remains within the expected Hugging Face model cache hierarchy.

Do not recursively follow arbitrary filesystem links merely to probe whether a cached artifact exists.

---

## Rust and Build Expectations

ModelVault is written in Rust.

The project is developed and tested on Windows as a first-class platform.

### Required validation

Before considering a change complete, run:

```powershell
.\scripts\Validate-ModelVault.ps1
```

Equivalent core commands are:

```powershell
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

If the lockfile is missing or stale, refresh it deliberately first.

For S3 validation:

```powershell
.\scripts\Validate-ModelVault.ps1 -WithS3
```

Or:

```powershell
cargo build --locked --features s3
```

### Cargo.lock

ModelVault is an application. Keep `Cargo.lock` committed.

Do not casually delete or exclude it from release archives.

### Clippy

Treat Clippy warnings as errors:

```powershell
cargo clippy --locked --all-targets -- -D warnings
```

Do not silence Clippy with `#[allow(...)]` merely to get a clean build unless there is a documented and justified reason.
Prefer fixing the design or code.

Examples of prior issues that should not be reintroduced:

- needless clones
- too many function arguments where an options struct is clearer
- impossible comparisons such as comparing `u8` to `256`
- unused imports
- needless lifetimes
- derivable defaults implemented manually
- style constructs that trigger strict Clippy

---

## Dependency Policy

Prefer small, well-maintained dependencies.

Do not add a new HTTP/TLS stack just to implement functionality already available through an existing supported tool or dependency unless there is a clear reason.

For example, Hugging Face downloading currently delegates to the official `hf` CLI rather than adding another large networking stack to ModelVault.

When adding dependencies:

1. justify why the dependency is needed
2. prefer feature-gating heavy optional integrations
3. update `Cargo.lock`
4. run locked validation
5. consider security/licensing impact

Recommended security tooling when available:

```powershell
cargo audit
cargo deny check
```

Do not claim these checks passed unless they were actually run.

---

## Security Principles

Read `SECURITY.md` before changing trust-boundary code.

Key principles:

1. Git-controlled metadata is untrusted input.
2. Remote objects are untrusted until content-verified.
3. Artifact/object hashes are integrity anchors.
4. Validate before filesystem mutation.
5. Validate before allocation.
6. Bound decompression and traversal work.
7. Use checked arithmetic for offsets and sizes.
8. Do not construct shell command strings from user input.
9. Pass CLI arguments separately through `Command`.
10. Use `--` before Git path arguments where applicable.
11. Do not store credentials in repository files.
12. Prefer secure defaults, but do not silently break legitimate local-development workflows.

---

## Command Invocation Safety

When invoking external programs such as Git or `hf`, use:

```rust
std::process::Command
```

with separate arguments.

Do NOT build command strings for `cmd.exe`, PowerShell, Bash, or `sh` from user input unless the feature explicitly requires shell interpretation and has a threat-model review.

For Git paths, use `--` where supported so filenames cannot be interpreted as options.

---

## Error Handling

Prefer actionable errors that state:

- what failed
- what path/object/artifact was involved
- what invariant was violated
- what the user can do next when obvious

Do not silently recover from integrity failures.

Examples that should be fatal unless explicitly operating in a diagnostic mode:

- object hash mismatch
- artifact hash mismatch
- manifest/pointer identity mismatch
- pack range outside file
- delta reconstruction mismatch
- unsafe path traversal
- lineage cycle creation

Missing optional metadata may be recoverable.
Missing lineage ancestors may be shown as unresolved rather than treated as corruption when the graph edge itself is valid.

---

## Tests

Every bug fix should ideally include a regression test.

Tests should emphasize invariants and boundary cases, including:

- Windows path normalization
- `\\?\` path behavior
- path traversal attempts
- absolute path rejection
- symlink/junction behavior
- malformed `.mvptr`
- pointer/manifest mismatches
- corrupt manifests
- checked integer overflow
- malicious pack indexes
- decompression overrun
- corrupt deltas
- lineage cycles
- missing lineage ancestors
- identical artifacts producing identical IDs
- storage representation changes preserving logical identity

Avoid brittle tests that compare path strings using mixed Windows/Unix separators.
Use `Path` semantics where possible.

Test data intended to be incompressible should use deterministic pseudo-random data rather than repetitive modulo patterns that Zstd can compress unexpectedly well.

---

## Documentation Rules

Documentation drift is a known project risk.

When changing behavior, update the authoritative documentation in the same change.

At minimum consider:

- `README.md`
- `CHANGELOG.md`
- `SECURITY.md`
- `docs/storage-format.md`
- `docs/remotes.md`
- `docs/provenance.md`
- `docs/lineage.md`
- command help text

### README

Keep the README focused on the current release and current workflows.
Do not turn it into a release-by-release historical log.

### CHANGELOG

Keep release entries chronological and unique.
Do not duplicate version headings.

### Storage-format documentation

If a physical format changes, document:

- version
- encoding
- identity semantics
- compatibility
- migration/read behavior

### Security documentation

If a change alters a trust boundary, threat model, remote behavior, credential handling, or integrity guarantee, update `SECURITY.md`.

---

## Versioning and Release Discipline

Use semantic versioning pragmatically:

- patch release: bug fixes, Clippy/build fixes, documentation corrections, non-format hardening
- minor release: backward-compatible features
- major/repository-format migration: incompatible behavior or storage/pointer/manifest changes requiring explicit migration

Before bumping a version:

1. inspect current `Cargo.toml`
2. inspect `Cargo.lock`
3. update `CHANGELOG.md`
4. verify docs
5. run the validation suite

Do not claim a release builds or tests successfully unless those commands were actually run in the current environment.

If the current agent environment lacks Rust/Cargo, say so explicitly and leave the final compile/test/Clippy gate to a Rust-enabled environment.

---

## Backward Compatibility

Prefer additive optional metadata over incompatible schema changes.

Examples:

- provenance is optional
- lineage is optional
- older manifests without new optional fields should remain readable

When adding fields to serialized Rust structures, consider `serde` defaults and compatibility with old files.

Never change the meaning of an existing serialized field casually.

Repository readers should generally be more tolerant than writers:

- readers may support older formats
- writers should emit the current canonical format

---

## Physical Optimization Rules

Commands such as `optimize`, pack compaction, and delta optimization may change physical storage representation.

They MUST NOT change:

- logical artifact bytes
- artifact ID
- object IDs
- pointer logical identity
- provenance meaning
- lineage meaning

Optimization should be idempotent when the repository is already in the optimal/current representation.

A repeated `optimize` on an already normalized repository should prefer a no-op rather than rewriting equivalent packs unnecessarily.

---

## Garbage Collection

Garbage collection must operate from logical reachability, not merely from physical-file presence.

Reachability must account for:

- manifests
- referenced objects
- delta base dependencies
- any future dependency relationships required for reconstruction

Do not collect a delta base that is still required to reconstruct a reachable target object.

Prefer dry-run or conservative reporting before destructive pruning.

---

## Performance Work

Do not optimize by weakening integrity checks unless the behavior is explicitly separated into fast and deep modes.

When measuring storage efficiency, distinguish:

- logical bytes
- unique reachable logical bytes
- dedup savings
- compression savings
- delta savings
- metadata overhead
- duplicate physical representations
- net physical savings

Avoid ambiguous metrics such as calling physical compression savings "dedup savings."

Use benchmark and simulation commands for storage-policy changes where possible.

---

## Agent Workflow

For a normal task, follow this sequence:

1. Read this file.
2. Read `README.md`, `SECURITY.md`, and relevant docs.
3. Inspect the current code instead of assuming architecture from memory.
4. Identify invariants affected by the requested change.
5. Make the smallest coherent implementation.
6. Add or update tests.
7. Update documentation when behavior changes.
8. Update version/changelog only when requested or when preparing a release.
9. Run validation.
10. Summarize exactly what changed and what was actually verified.

For security-sensitive changes, explicitly identify:

- trust boundary
- attacker-controlled input
- validation point
- resource bound
- integrity check
- compatibility consequence

---

## Do Not Do These Things

Agents MUST NOT casually:

- change BLAKE3 logical identity semantics
- hash compressed bytes instead of logical bytes
- make provenance or lineage part of artifact identity
- allow pointer-controlled arbitrary filesystem paths
- trust manifest sizes before validation
- use unbounded decompression on untrusted data
- allow pack indexes to escape `.modelvault/packs/`
- use PID-only temp filenames in attacker-writable locations
- shell-interpolate Git/Hugging Face arguments
- store AWS credentials in repository config
- make the AWS SDK mandatory for default builds without explicit approval
- remove backward compatibility for older manifests without a migration plan
- silently rewrite repository metadata defaults over explicit existing configuration
- change physical storage in a way that changes logical object IDs
- delete `Cargo.lock` from application releases
- suppress Clippy warnings just to pass `-D warnings`
- claim tests/builds passed when they were not actually run

---

## Useful Validation Commands

### Full default validation

```powershell
.\scripts\Validate-ModelVault.ps1
```

### Manual default validation

```powershell
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

### Optional S3 validation

```powershell
.\scripts\Validate-ModelVault.ps1 -WithS3
```

### Repository validation

```powershell
cargo run -- repo-info
cargo run -- fsck
cargo run -- fsck --deep
cargo run -- pack-verify
```

### Physical-storage validation

```powershell
cargo run -- storage
cargo run -- analytics --detailed
cargo run -- optimize --dry-run
```

### Model metadata

```powershell
cargo run -- provenance .\models\<model>\model.safetensors.mvptr
cargo run -- lineage .\models\<model>\model.safetensors.mvptr
```

### CLI sanity

```powershell
cargo run -- -V
cargo run -- help
```

---

## Current Compatibility Notes

ModelVault has historically been exercised on Windows with Rust stable and strict Clippy.

Important lessons from prior regressions:

- large Clap command graphs can exhaust the default Windows main-thread stack in debug builds; CLI parsing/dispatch currently uses a larger dedicated thread stack
- Windows canonicalization may return `\\?\` paths
- Hugging Face caches may contain symlinks/link-like snapshot entries
- AWS Rust SDK builds can be memory-intensive on Windows
- compressed test fixtures can produce surprising results if they are structurally repetitive

Do not "simplify" fixes for these issues without understanding why they exist.

---

## Definition of Done

A change is complete when all applicable items are true:

- implementation matches the requested behavior
- architectural invariants remain intact
- security boundaries remain intact or are stronger
- regression tests exist for important fixes
- documentation matches behavior
- serialization compatibility has been considered
- `Cargo.lock` is current
- default build passes
- tests pass
- strict Clippy passes
- optional S3 path is validated when touched
- no claim is made about validation that was not actually executed

For releases, additionally ensure:

- version is correct
- changelog is correct
- release archive contains the full source tree
- archive contains `Cargo.lock`
- archive contains `README.md`, `SECURITY.md`, and `AGENTS.md`

---

## Final Guidance for Coding Agents

ModelVault's most important design property is the separation between **logical artifact identity** and **physical storage representation**.

When uncertain, preserve this principle:

```text
logical bytes determine identity
metadata explains the artifact
physical storage optimizes the artifact
Git records the lightweight history
```

Changes that maintain this separation are usually aligned with the architecture.
Changes that blur these boundaries require extra scrutiny, migration planning, and tests.
