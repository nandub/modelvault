use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactProvenance {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactLineageEdge {
    pub parent_artifact_id: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    pub object: String,
    pub offset: u64,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tensor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TensorManifest {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<usize>,
    pub data_offset: u64,
    pub data_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub version: u32,
    pub artifact_id: String,
    pub format: String,
    pub source_name: String,
    pub logical_size: u64,
    pub chunk_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ArtifactProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lineage: Vec<ArtifactLineageEdge>,
    pub chunks: Vec<ChunkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tensors: Vec<TensorManifest>,
}

impl ArtifactManifest {
    pub fn manifest_path(store: &Path, artifact_id: &str) -> PathBuf {
        store.join("manifests").join(format!("{artifact_id}.json"))
    }

    pub fn save(&self, store: &Path) -> io::Result<PathBuf> {
        validate_manifest_structure(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let dir = store.join("manifests");
        fs::create_dir_all(&dir)?;
        let path = Self::manifest_path(store, &self.artifact_id);
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        fs::write(&path, bytes)?;
        Ok(path)
    }

    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        let manifest: Self = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        validate_manifest_structure(&manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(manifest)
    }
}

/// Validate untrusted manifest metadata before it can influence filesystem
/// allocation, object reads, or materialization offsets.
pub fn validate_manifest_structure(manifest: &ArtifactManifest) -> anyhow::Result<()> {
    ensure!(
        manifest.version == 1,
        "unsupported manifest version {}",
        manifest.version
    );
    ensure!(
        manifest.artifact_id.len() == 64
            && manifest.artifact_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid artifact BLAKE3 id"
    );

    ensure!(
        manifest.chunk_size > 0,
        "manifest chunk_size must be greater than zero"
    );

    for edge in &manifest.lineage {
        ensure!(
            edge.parent_artifact_id.len() == 64
                && edge
                    .parent_artifact_id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit()),
            "invalid lineage parent BLAKE3 id"
        );
        ensure!(
            !edge
                .parent_artifact_id
                .eq_ignore_ascii_case(&manifest.artifact_id),
            "artifact lineage cannot reference itself"
        );
        let operation = edge.operation.trim();
        ensure!(!operation.is_empty(), "lineage operation cannot be empty");
        ensure!(
            operation.len() <= 64,
            "lineage operation cannot exceed 64 bytes"
        );
        if let Some(note) = &edge.note {
            ensure!(note.len() <= 1024, "lineage note cannot exceed 1024 bytes");
        }
    }
    let mut expected = 0u64;
    for chunk in &manifest.chunks {
        ensure!(chunk.size > 0, "manifest chunks must have non-zero size");
        ensure!(
            chunk.object.len() == 64 && chunk.object.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid chunk BLAKE3 id"
        );
        ensure!(
            chunk.offset == expected,
            "manifest gap/overlap: expected offset {}, found {}",
            expected,
            chunk.offset
        );
        expected = expected
            .checked_add(chunk.size)
            .context("manifest byte range overflow")?;
    }
    ensure!(
        expected == manifest.logical_size,
        "manifest covers {} bytes, expected {}",
        expected,
        manifest.logical_size
    );

    for tensor in &manifest.tensors {
        let end = tensor
            .data_offset
            .checked_add(tensor.data_size)
            .context("tensor byte range overflow")?;
        ensure!(
            end <= manifest.logical_size,
            "tensor '{}' extends beyond artifact",
            tensor.name
        );
    }
    Ok(())
}
