use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};

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
