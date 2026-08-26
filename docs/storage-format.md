# ModelVault storage format

This document describes the storage model used by ModelVault 1.5.x. Logical identity is intentionally independent from physical representation.

## Artifact ID

`artifact_id` is the lowercase 64-character BLAKE3 digest of the complete original artifact bytes.

## CAS object ID

Each CAS object is addressed by the lowercase BLAKE3 digest of its **decoded logical bytes**. The same object ID is preserved whether the object is stored raw, Zstandard-compressed, in a pack, or as a persistent delta.

## Manifest v1 coverage invariant

For a valid v1 manifest in stored order:

- the first chunk begins at offset `0`;
- every next chunk begins exactly after the preceding chunk;
- chunk offset/size arithmetic must not overflow;
- the final chunk ends exactly at `logical_size`;
- every chunk object ID is a 64-character hexadecimal BLAKE3 identifier;
- tensor ranges must remain inside `logical_size`.

ModelVault validates this structure before materialization.

## Safetensors

Safetensors consists of an 8-byte little-endian header length, a JSON header, and a contiguous tensor data buffer. ModelVault stores the binary header region with ordinary chunks and chunks tensor payloads without crossing tensor boundaries. Exact original bytes, including the original header representation, are preserved for byte-identical reconstruction.

## Repository metadata

Physical-layout policy lives in `.modelvault/repository.json`:

```json
{
  "version": 1,
  "object_hash": "blake3",
  "loose_compression": "none",
  "zstd_level": 3,
  "pack_format_version": 2,
  "delta_min_savings_pct": 20,
  "max_delta_depth": 2
}
```

These settings do not change artifact IDs, object IDs, manifests, or `.mvptr` identity semantics.

## Loose objects

Raw loose objects live under:

```text
.modelvault/objects/<first-2-hex>/<remaining-62-hex>
```

When loose compression is `zstd`, the physical file uses the `MVZ1` envelope:

```text
4 bytes   ASCII "MVZ1"
8 bytes   decoded logical length, little-endian u64
N bytes   Zstandard frame
```

Decompression is bounded by the declared logical length and the result must have exactly that length.

## Pack files

Pack files live under `.modelvault/packs/`:

```text
pack-<id>.mvpack
pack-<id>.idx.json
```

### Pack v1

Pack v1 indexes raw logical bytes using offset and logical size. ModelVault retains read compatibility with v1 indexes.

### Pack v2

Pack v2 is the current write format. Each index entry contains:

- object ID;
- byte offset in the pack;
- logical decoded size (`size`);
- stored physical size (`stored_size`);
- encoding (`raw` or `zstd`).

Each packed object is decoded independently and validated against its object ID. Pack v2 allows compression without changing the logical CAS namespace.

## Persistent delta object (`MVD1`)

A delta is an optional physical representation for an existing logical CAS object:

```text
.modelvault/deltas/<first-2-hex>/<remaining-62-hex>.mvdelta
```

Binary layout:

```text
Offset  Size  Meaning
0       4     ASCII magic `MVD1`
4       64    lowercase hexadecimal BLAKE3 ID of base object
68      1     delta chain depth
69      8     target logical byte length, little-endian u64
77      ...   Zstandard-compressed XOR bytes
```

Reconstruction is:

```text
target = base XOR zstd_decode(payload)
```

The delta payload is decoded with a hard output bound equal to the declared target logical size. The reconstructed bytes must hash to the target object ID. Dependency cycles are rejected and repository policy bounds maximum delta depth.

## Pointer format

A `.mvptr` is Git-facing metadata. The generated manifest reference is exactly:

```text
.modelvault/manifests/<artifact-id>.json
```

When a pointer is resolved, ModelVault validates that this path matches the pointer artifact ID and that pointer identity/size/format/source metadata agrees with the manifest. Arbitrary path references are rejected.

## Physical normalization

`modelvault optimize` chooses the best known primary representation among pack-v2 full encodings and retained smaller deltas, removes redundant physical copies, and verifies logical objects after cleanup. Manifests remain unchanged.


## Lineage metadata

Manifest v1 and pointer v1 may optionally contain a `lineage` array. Each entry records `parent_artifact_id`, `operation`, and an optional `note`. The field defaults to an empty array when absent, so pre-1.6 files remain readable. Lineage changes metadata only; logical artifact IDs and CAS object IDs continue to be BLAKE3 hashes of artifact/object bytes.
