# Artifact lineage

ModelVault 1.6 adds optional derivation lineage to artifact manifests and pointers. Lineage is metadata about immutable artifact identities; it does not change BLAKE3 artifact IDs, chunk IDs, CAS representation, packs, or deltas.

## Edge schema

Each child artifact may contain zero or more parent edges:

```json
{
  "parent_artifact_id": "<64-hex BLAKE3 artifact id>",
  "operation": "fine-tune",
  "note": "optional human-readable context"
}
```

Common operation labels include `fine-tune`, `quantize`, `convert`, `merge`, `distill`, `prune`, and `extract-tensors`. The schema intentionally keeps the label extensible rather than defining a closed enum.

## Recording a derivation

```powershell
modelvault derive .\models\child\model.safetensors.mvptr `
    --parent .\models\base\model.safetensors.mvptr `
    --operation fine-tune `
    --note "training run 42" `
    --stage
```

`derive` requires both child and parent manifests to be locally resolvable. It refuses self-parent relationships and checks the parent's known ancestry before writing the edge so it cannot create a cycle. Repeating the same edge is idempotent.

## Inspecting ancestry

```powershell
modelvault lineage .\models\child\model.safetensors.mvptr
modelvault lineage .\models\child\model.safetensors.mvptr --json
modelvault lineage .\models\child\model.safetensors.mvptr --max-depth 8
```

Human-readable output is a parent tree annotated with the operation on each edge. JSON output is a recursive graph representation suitable for automation.

If a recorded parent manifest is absent from the local repository, ModelVault preserves that edge and reports the parent node as `missing: true`. This allows partial clones/remotes to retain lineage identity without fabricating metadata.

## Safety bounds

- Parent IDs must be 64 hexadecimal BLAKE3 artifact IDs.
- An artifact cannot reference itself.
- Operation labels are non-empty and limited to 64 bytes.
- Notes are limited to 1024 bytes.
- CLI display depth is limited to 256.
- Cycle-prevention traversal has a 100,000-node safety ceiling.

## Metadata semantics

Lineage is mutable metadata attached to a content-addressed artifact manifest. If byte-identical artifacts are encountered through multiple derivation paths, ModelVault may accumulate multiple known parent edges for the same artifact identity. This reflects known provenance/derivation relationships without duplicating artifact bytes.
