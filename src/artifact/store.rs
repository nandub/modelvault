use std::{collections::BTreeMap, fs::File, io::{Read, Seek, SeekFrom}, path::Path};

use anyhow::{bail, Context};
use serde::Deserialize;

use crate::{
    cas::LocalCas,
    chunk::{chunk_bytes, chunk_tensor_range},
    manifest::{ArtifactManifest, ChunkRef, TensorManifest},
};

#[derive(Debug, Clone)]
pub struct AddArtifactResult {
    pub manifest: ArtifactManifest,
    pub new_bytes: u64,
    pub reused_bytes: u64,
    pub manifest_path: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
struct RawTensorHeader {
    dtype: String,
    shape: Vec<usize>,
    data_offsets: [u64; 2],
}

pub fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn add_raw_artifact(path: &Path, cas: &LocalCas, chunk_size: usize) -> anyhow::Result<AddArtifactResult> {
    let logical_size = std::fs::metadata(path)?.len();
    let artifact_id = hash_file(path)?;
    let mut file = File::open(path)?;
    let mut chunks = Vec::new();
    let mut new_bytes = 0u64;
    let mut reused_bytes = 0u64;

    for range in chunk_bytes(logical_size, chunk_size) {
        file.seek(SeekFrom::Start(range.offset))?;
        let mut data = vec![0u8; range.len];
        file.read_exact(&mut data)?;
        let put = cas.put_bytes(&data)?;
        if put.was_new { new_bytes += put.size; } else { reused_bytes += put.size; }
        chunks.push(ChunkRef {
            object: put.id.to_string(),
            offset: range.offset,
            size: range.len as u64,
            tensor: None,
        });
    }

    let manifest = ArtifactManifest {
        version: 1,
        artifact_id,
        format: "raw".into(),
        source_name: path.file_name().and_then(|v| v.to_str()).unwrap_or("artifact").to_owned(),
        logical_size,
        chunk_size: chunk_size as u64,
        provenance: None,
        lineage: Vec::new(),
        chunks,
        tensors: Vec::new(),
    };
    let manifest_path = manifest.save(cas.root())?;
    Ok(AddArtifactResult { manifest, new_bytes, reused_bytes, manifest_path })
}

pub fn add_safetensors_artifact(path: &Path, cas: &LocalCas, chunk_size: usize) -> anyhow::Result<AddArtifactResult> {
    let logical_size = std::fs::metadata(path)?.len();
    if logical_size < 8 { bail!("Safetensors file is too small to contain a header"); }

    let artifact_id = hash_file(path)?;
    let mut file = File::open(path)?;
    let mut prefix = [0u8; 8];
    file.read_exact(&mut prefix)?;
    let header_len = u64::from_le_bytes(prefix);
    let data_start = 8u64.checked_add(header_len).context("Safetensors header length overflow")?;
    if data_start > logical_size { bail!("Safetensors header extends beyond end of file"); }

    let mut header_bytes = vec![0u8; header_len as usize];
    file.read_exact(&mut header_bytes)?;
    let value: serde_json::Value = serde_json::from_slice(&header_bytes)
        .context("Invalid Safetensors JSON header")?;
    let map = value.as_object().context("Safetensors header must be a JSON object")?;

    let mut tensor_headers = BTreeMap::new();
    for (name, v) in map {
        if name == "__metadata__" { continue; }
        let raw: RawTensorHeader = serde_json::from_value(v.clone())
            .with_context(|| format!("Invalid Safetensors tensor header for '{name}'"))?;
        if raw.data_offsets[1] < raw.data_offsets[0] { bail!("Invalid data offsets for tensor '{name}'"); }
        tensor_headers.insert(name.clone(), raw);
    }

    // Validate with the reference parser as well. This catches overlaps and malformed tensors.
    crate::artifact::inspect_safetensors(path)?;

    let mut chunks = Vec::new();
    let mut tensors = Vec::new();
    let mut new_bytes = 0u64;
    let mut reused_bytes = 0u64;

    // Preserve the complete binary header as ordinary chunks so materialization is byte-identical.
    for range in chunk_bytes(data_start, chunk_size) {
        file.seek(SeekFrom::Start(range.offset))?;
        let mut data = vec![0u8; range.len];
        file.read_exact(&mut data)?;
        let put = cas.put_bytes(&data)?;
        if put.was_new { new_bytes += put.size; } else { reused_bytes += put.size; }
        chunks.push(ChunkRef { object: put.id.to_string(), offset: range.offset, size: range.len as u64, tensor: None });
    }

    for (name, raw) in tensor_headers {
        let relative_start = raw.data_offsets[0];
        let relative_end = raw.data_offsets[1];
        let tensor_len = relative_end - relative_start;
        let absolute_start = data_start + relative_start;
        let absolute_end = data_start + relative_end;
        if absolute_end > logical_size { bail!("Tensor '{name}' extends beyond end of file"); }

        tensors.push(TensorManifest {
            name: name.clone(), dtype: raw.dtype, shape: raw.shape,
            data_offset: absolute_start, data_size: tensor_len,
        });
        for tc in chunk_tensor_range(name.clone(), absolute_start, tensor_len, chunk_size) {
            file.seek(SeekFrom::Start(tc.absolute.offset))?;
            let mut data = vec![0u8; tc.absolute.len];
            file.read_exact(&mut data)?;
            let put = cas.put_bytes(&data)?;
            if put.was_new { new_bytes += put.size; } else { reused_bytes += put.size; }
            chunks.push(ChunkRef {
                object: put.id.to_string(), offset: tc.absolute.offset,
                size: tc.absolute.len as u64, tensor: Some(name.clone()),
            });
        }
    }

    chunks.sort_by_key(|c| c.offset);
    // Safetensors should have contiguous tensor payloads. Verify the manifest covers every byte exactly once.
    let mut expected = 0u64;
    for c in &chunks {
        if c.offset != expected { bail!("Manifest coverage gap/overlap at byte {expected}, next chunk begins at {}", c.offset); }
        expected = expected.checked_add(c.size).context("Manifest size overflow")?;
    }
    if expected != logical_size { bail!("Manifest covers {expected} bytes but artifact contains {logical_size} bytes"); }

    let manifest = ArtifactManifest {
        version: 1,
        artifact_id,
        format: "safetensors".into(),
        source_name: path.file_name().and_then(|v| v.to_str()).unwrap_or("model.safetensors").to_owned(),
        logical_size,
        chunk_size: chunk_size as u64,
        provenance: None,
        lineage: Vec::new(),
        chunks,
        tensors,
    };
    let manifest_path = manifest.save(cas.root())?;
    Ok(AddArtifactResult { manifest, new_bytes, reused_bytes, manifest_path })
}
