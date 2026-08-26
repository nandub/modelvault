# ModelVault provenance

ModelVault records optional stable-source provenance for artifacts imported from a source that has a portable identity. The first provider supported is Hugging Face.

## Design rules

- Provenance does not participate in the BLAKE3 artifact ID. The artifact ID remains the hash of the artifact bytes.
- Provenance is optional and backward-compatible with existing manifest and pointer version 1 files.
- Machine-specific cache paths are not persisted as provenance.
- Hugging Face snapshot commit IDs are preferred as the resolved revision when the cached path exposes one.

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
