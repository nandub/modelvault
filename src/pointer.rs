use std::{fs, io, path::{Path, PathBuf}};
use serde::{Deserialize, Serialize};

use crate::manifest::{ArtifactLineageEdge, ArtifactManifest, ArtifactProvenance};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactPointer {
    pub version: u32,
    pub artifact_id: String,
    pub format: String,
    pub logical_size: u64,
    pub manifest: String,
    pub source_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ArtifactProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lineage: Vec<ArtifactLineageEdge>,
}

impl ArtifactPointer {
    pub fn from_manifest(manifest: &ArtifactManifest) -> Self {
        Self {
            version: 1,
            artifact_id: manifest.artifact_id.clone(),
            format: manifest.format.clone(),
            logical_size: manifest.logical_size,
            manifest: format!(".modelvault/manifests/{}.json", manifest.artifact_id),
            source_name: manifest.source_name.clone(),
            provenance: manifest.provenance.clone(),
            lineage: manifest.lineage.clone(),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(path, bytes)
    }

    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn default_pointer_path(source: &Path) -> PathBuf {
        let mut os = source.as_os_str().to_owned();
        os.push(".mvptr");
        PathBuf::from(os)
    }

    /// Resolve and validate the manifest referenced by this Git-controlled
    /// pointer. Pointer paths are deliberately restricted to ModelVault's
    /// content-addressed manifest namespace; arbitrary paths are rejected.
    pub fn resolve_manifest(&self, repository_root: &Path) -> anyhow::Result<(PathBuf, ArtifactManifest)> {
        anyhow::ensure!(self.version == 1, "unsupported pointer version {}", self.version);
        anyhow::ensure!(
            self.artifact_id.len() == 64 && self.artifact_id.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid pointer artifact BLAKE3 id"
        );
        let expected_manifest = format!(".modelvault/manifests/{}.json", self.artifact_id.to_ascii_lowercase());
        anyhow::ensure!(
            self.manifest == expected_manifest,
            "pointer manifest must be '{}'",
            expected_manifest
        );

        let manifest_path = repository_root
            .join(".modelvault")
            .join("manifests")
            .join(format!("{}.json", self.artifact_id.to_ascii_lowercase()));
        let manifest = ArtifactManifest::load(&manifest_path)?;
        anyhow::ensure!(
            manifest.artifact_id.eq_ignore_ascii_case(&self.artifact_id),
            "pointer artifact ID does not match referenced manifest"
        );
        anyhow::ensure!(
            manifest.logical_size == self.logical_size,
            "pointer logical size {} does not match manifest logical size {}",
            self.logical_size,
            manifest.logical_size
        );
        anyhow::ensure!(
            manifest.format == self.format,
            "pointer format '{}' does not match manifest format '{}'",
            self.format,
            manifest.format
        );
        anyhow::ensure!(
            manifest.source_name == self.source_name,
            "pointer source name '{}' does not match manifest source name '{}'",
            self.source_name,
            manifest.source_name
        );
        if let Some(pointer_provenance) = &self.provenance {
            anyhow::ensure!(
                manifest.provenance.as_ref() == Some(pointer_provenance),
                "pointer provenance does not match referenced manifest"
            );
        }
        if !self.lineage.is_empty() {
            anyhow::ensure!(
                manifest.lineage == self.lineage,
                "pointer lineage does not match referenced manifest"
            );
        }
        Ok((manifest_path, manifest))
    }
}
