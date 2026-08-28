use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, ensure, Context};
use serde::Serialize;

use crate::{
    cas::{LocalCas, ObjectId},
    manifest::{validate_manifest_structure, ArtifactManifest},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn create_temp_file_near(output: &Path) -> anyhow::Result<(PathBuf, File)> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let stem = output.file_name().and_then(|v| v.to_str()).unwrap_or("artifact");
    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = parent.join(format!(".{stem}.modelvault-tmp-{}-{nanos:x}-{sequence:x}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
    anyhow::bail!("unable to create a unique temporary materialization file")
}

pub fn materialize(manifest: &ArtifactManifest, cas: &LocalCas, output: &Path) -> anyhow::Result<()> {
    validate_manifest_structure(manifest)?;
    if let Some(parent) = output.parent() { fs::create_dir_all(parent)?; }
    let (tmp, mut file) = create_temp_file_near(output)?;
    file.set_len(manifest.logical_size)?;

    for chunk in &manifest.chunks {
        let id = ObjectId::parse(&chunk.object)?;
        let bytes = cas.read(&id).with_context(|| format!("Missing CAS object {}", chunk.object))?;
        if bytes.len() as u64 != chunk.size {
            bail!("CAS object {} has {} bytes; manifest expects {}", chunk.object, bytes.len(), chunk.size);
        }
        if !cas.verify(&id)? { bail!("CAS object {} failed BLAKE3 verification", chunk.object); }
        file.seek(SeekFrom::Start(chunk.offset))?;
        file.write_all(&bytes)?;
    }
    file.sync_all()?;
    drop(file);

    let actual = super::store::hash_file(&tmp)?;
    if actual != manifest.artifact_id {
        let _ = fs::remove_file(&tmp);
        bail!("Materialized artifact hash mismatch: expected {}, got {}", manifest.artifact_id, actual);
    }

    if output.exists() { fs::remove_file(output)?; }
    fs::rename(tmp, output)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectedTensorMaterialization {
    pub source_artifact_id: String,
    pub derived_artifact_id: String,
    pub tensor_count: usize,
    pub logical_size: u64,
}

#[derive(Serialize)]
struct SafetensorsHeader {
    dtype: String,
    shape: Vec<usize>,
    data_offsets: [u64; 2],
}

fn verify_source_identity(manifest: &ArtifactManifest, cas: &LocalCas) -> anyhow::Result<()> {
    let mut hasher = blake3::Hasher::new();
    for chunk in &manifest.chunks {
        let id = ObjectId::parse(&chunk.object)?;
        let bytes = cas.read(&id).with_context(|| format!("Missing CAS object {}", chunk.object))?;
        ensure!(bytes.len() as u64 == chunk.size, "CAS object {} has {} bytes; manifest expects {}", chunk.object, bytes.len(), chunk.size);
        ensure!(cas.verify(&id)?, "CAS object {} failed BLAKE3 verification", chunk.object);
        hasher.update(&bytes);
    }
    ensure!(hasher.finalize().to_hex().as_str() == manifest.artifact_id, "source artifact hash mismatch: expected {}", manifest.artifact_id);
    Ok(())
}

fn write_tensor_range(file: &mut File, manifest: &ArtifactManifest, cas: &LocalCas, start: u64, size: u64, hasher: &mut blake3::Hasher) -> anyhow::Result<()> {
    let end = start.checked_add(size).context("selected tensor range overflow")?;
    for chunk in &manifest.chunks {
        let chunk_end = chunk.offset.checked_add(chunk.size).context("manifest chunk range overflow")?;
        let overlap_start = start.max(chunk.offset);
        let overlap_end = end.min(chunk_end);
        if overlap_start >= overlap_end { continue; }
        let id = ObjectId::parse(&chunk.object)?;
        let bytes = cas.read(&id).with_context(|| format!("Missing CAS object {}", chunk.object))?;
        ensure!(bytes.len() as u64 == chunk.size, "CAS object {} has {} bytes; manifest expects {}", chunk.object, bytes.len(), chunk.size);
        ensure!(cas.verify(&id)?, "CAS object {} failed BLAKE3 verification", chunk.object);
        let from = usize::try_from(overlap_start - chunk.offset).context("selected tensor offset exceeds platform limits")?;
        let to = usize::try_from(overlap_end - chunk.offset).context("selected tensor range exceeds platform limits")?;
        file.write_all(&bytes[from..to])?;
        hasher.update(&bytes[from..to]);
    }
    Ok(())
}

pub fn materialize_selected_safetensors(manifest: &ArtifactManifest, cas: &LocalCas, names: &[String], output: &Path) -> anyhow::Result<SelectedTensorMaterialization> {
    validate_manifest_structure(manifest)?;
    ensure!(manifest.format == "safetensors", "tensor selection requires a Safetensors artifact");
    ensure!(!names.is_empty(), "at least one --tensor is required");
    verify_source_identity(manifest, cas)?;

    let requested: HashSet<&str> = names.iter().map(String::as_str).collect();
    ensure!(requested.len() == names.len(), "duplicate tensor selections are not allowed");
    let selected = manifest.tensors.iter().filter(|tensor| requested.contains(tensor.name.as_str())).collect::<Vec<_>>();
    ensure!(selected.len() == requested.len(), "one or more selected tensors do not exist in the source artifact");

    let mut offsets = BTreeMap::new();
    let mut data_size = 0u64;
    for tensor in &selected {
        let end = data_size.checked_add(tensor.data_size).context("selected Safetensors size overflow")?;
        offsets.insert(tensor.name.clone(), SafetensorsHeader { dtype: tensor.dtype.clone(), shape: tensor.shape.clone(), data_offsets: [data_size, end] });
        data_size = end;
    }
    let header = serde_json::to_vec(&offsets)?;
    let header_len = u64::try_from(header.len()).context("selected Safetensors header exceeds u64")?;
    let logical_size = 8u64.checked_add(header_len).and_then(|value| value.checked_add(data_size)).context("selected Safetensors size overflow")?;

    if let Some(parent) = output.parent() { fs::create_dir_all(parent)?; }
    let (tmp, mut file) = create_temp_file_near(output)?;
    let result = (|| -> anyhow::Result<SelectedTensorMaterialization> {
        let mut hasher = blake3::Hasher::new();
        let header_prefix = header_len.to_le_bytes();
        file.write_all(&header_prefix)?;
        file.write_all(&header)?;
        hasher.update(&header_prefix);
        hasher.update(&header);
        for tensor in selected {
            write_tensor_range(&mut file, manifest, cas, tensor.data_offset, tensor.data_size, &mut hasher)?;
        }
        file.sync_all()?;
        ensure!(file.metadata()?.len() == logical_size, "selected Safetensors output size mismatch");
        Ok(SelectedTensorMaterialization { source_artifact_id: manifest.artifact_id.clone(), derived_artifact_id: hasher.finalize().to_hex().to_string(), tensor_count: names.len(), logical_size })
    })();
    drop(file);
    match result {
        Ok(result) => {
            if output.exists() { fs::remove_file(output)?; }
            fs::rename(tmp, output)?;
            Ok(result)
        }
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            Err(error)
        }
    }
}

pub fn verify_artifact(manifest: &ArtifactManifest, cas: &LocalCas) -> anyhow::Result<()> {
    let mut logical = 0u64;
    let mut expected_offset = 0u64;
    for chunk in &manifest.chunks {
        if chunk.offset != expected_offset {
            bail!("Manifest has a gap or overlap: expected offset {}, found {}", expected_offset, chunk.offset);
        }
        let id = ObjectId::parse(&chunk.object)?;
        let bytes = cas.read(&id).with_context(|| format!("Missing CAS object {}", chunk.object))?;
        if bytes.len() as u64 != chunk.size {
            bail!(
                "CAS object {} decoded to {} bytes; manifest expects {}",
                chunk.object,
                bytes.len(),
                chunk.size
            );
        }
        if !cas.verify(&id)? { bail!("CAS object {} failed BLAKE3 verification", chunk.object); }
        expected_offset = expected_offset.checked_add(chunk.size).context("manifest byte range overflow")?;
        logical = logical.checked_add(chunk.size).context("manifest logical size overflow")?;
    }
    if logical != manifest.logical_size { bail!("Manifest logical size mismatch: chunks={}, manifest={}", logical, manifest.logical_size); }
    Ok(())
}
