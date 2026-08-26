# ModelVault 1.5.0 security and documentation review

This release follows the pre-lineage review of ModelVault 1.4.1. It intentionally prioritizes hardening and documentation reconciliation over new model-management features.

## Remediated findings

| Finding | Resolution |
|---|---|
| `.mvptr` could name an arbitrary manifest path | Pointer manifest path is now restricted to `.modelvault/manifests/<artifact-id>.json`. |
| Pointer identity could disagree with manifest | Artifact ID, logical size, format, source name, and pointer provenance are cross-validated. |
| Materialization allocated output before full layout validation | Manifest structure is validated before parent creation/temp-file creation/`set_len`. |
| Zstd decode was unbounded before expected-size check | Loose, pack, and delta decoders stop at `expected + 1` bytes and reject mismatch. |
| PID-only temporary write paths | Materialization/CAS/pack writes use unique names plus `create_new` semantics. |
| Pack index could name a pack outside the packs directory | `pack_file` must be a single `.mvpack` basename; entries receive ID/range/raw-size validation. |
| Import `--to` could create directories before containment rejection | Nearest existing ancestor is canonicalized and checked before directory creation. |
| Hugging Face snapshot symlink target was not constrained | Canonical target must remain inside that model's Hugging Face cache directory. |
| S3 fast verification metadata trust was undocumented | `--deep-verify` adds body-level BLAKE3 verification and docs distinguish both modes. |
| Non-local custom S3 endpoint could silently use HTTP | CLI emits a plaintext warning for non-loopback HTTP endpoints. |
| README/storage/remotes/changelog drift | Current-state docs were rewritten/reconciled and `SECURITY.md` added. |

## Reproducible dependency state

`Cargo.lock` should be committed for application releases and locked validation should be mandatory. The archive-generation environment used for this package does not contain Cargo, so it cannot truthfully generate a dependency lockfile. `scripts/Validate-ModelVault.ps1` generates the lockfile on a Rust-enabled workstation if it is missing, then runs locked build/test/Clippy validation. The generated lockfile should be retained in source control before tagging a final release.

Recommended validation:

```powershell
.\scripts\Validate-ModelVault.ps1
```

Optional S3 and supply-chain checks:

```powershell
.\scripts\Validate-ModelVault.ps1 -WithS3 -SecurityTools
```

## Compatibility

The hardening changes do not intentionally alter artifact IDs, CAS object IDs, manifest-v1 schema, pointer-v1 schema, pack-v2 identity semantics, MVD1 identity semantics, or repository metadata version.

The pointer resolver is intentionally stricter: hand-written pointers that use nonstandard manifest paths or disagree with their manifests are now rejected rather than tolerated.
