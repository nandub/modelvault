use std::{collections::HashSet, path::Path};

use anyhow::{bail, ensure};
use serde::Serialize;

use crate::manifest::{ArtifactLineageEdge, ArtifactManifest, ArtifactProvenance};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LineageGraphEdge {
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub parent: Box<LineageGraphNode>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LineageGraphNode {
    pub artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ArtifactProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<LineageGraphEdge>,
    pub missing: bool,
    pub truncated: bool,
}

pub fn add_lineage_edge(
    child: &mut ArtifactManifest,
    parent: &ArtifactManifest,
    operation: &str,
    note: Option<&str>,
) -> anyhow::Result<bool> {
    let operation = operation.trim();
    ensure!(!operation.is_empty(), "--operation cannot be empty");
    ensure!(operation.len() <= 64, "--operation cannot exceed 64 bytes");
    if let Some(note) = note {
        ensure!(note.len() <= 1024, "--note cannot exceed 1024 bytes");
    }
    ensure!(
        !child.artifact_id.eq_ignore_ascii_case(&parent.artifact_id),
        "an artifact cannot derive from itself"
    );

    let edge = ArtifactLineageEdge {
        parent_artifact_id: parent.artifact_id.to_ascii_lowercase(),
        operation: operation.to_string(),
        note: note.map(ToOwned::to_owned),
    };
    if child.lineage.contains(&edge) {
        return Ok(false);
    }
    child.lineage.push(edge);
    crate::manifest::validate_manifest_structure(child)?;
    Ok(true)
}

pub fn ensure_no_lineage_cycle(
    store: &Path,
    parent: &ArtifactManifest,
    child_artifact_id: &str,
) -> anyhow::Result<()> {
    let mut pending = vec![parent.artifact_id.clone()];
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        let id = id.to_ascii_lowercase();
        if !seen.insert(id.clone()) {
            continue;
        }
        ensure!(
            !id.eq_ignore_ascii_case(child_artifact_id),
            "lineage edge would create a cycle through artifact {child_artifact_id}"
        );
        let path = ArtifactManifest::manifest_path(store, &id);
        if !path.is_file() {
            continue;
        }
        let manifest = ArtifactManifest::load(path)?;
        pending.extend(manifest.lineage.into_iter().map(|edge| edge.parent_artifact_id));
        ensure!(seen.len() <= 100_000, "lineage graph exceeds safety traversal limit");
    }
    Ok(())
}

pub fn build_lineage_graph(
    store: &Path,
    root: &ArtifactManifest,
    max_depth: usize,
) -> anyhow::Result<LineageGraphNode> {
    let mut stack = HashSet::new();
    build_node(store, root, 0, max_depth, &mut stack)
}

fn build_node(
    store: &Path,
    manifest: &ArtifactManifest,
    depth: usize,
    max_depth: usize,
    stack: &mut HashSet<String>,
) -> anyhow::Result<LineageGraphNode> {
    let id = manifest.artifact_id.to_ascii_lowercase();
    if !stack.insert(id.clone()) {
        bail!("lineage cycle detected at artifact {}", manifest.artifact_id);
    }

    let mut node = LineageGraphNode {
        artifact_id: manifest.artifact_id.clone(),
        source_name: Some(manifest.source_name.clone()),
        format: Some(manifest.format.clone()),
        provenance: manifest.provenance.clone(),
        parents: Vec::new(),
        missing: false,
        truncated: depth >= max_depth && !manifest.lineage.is_empty(),
    };

    if depth < max_depth {
        for edge in &manifest.lineage {
            let parent_path = ArtifactManifest::manifest_path(store, &edge.parent_artifact_id);
            let parent_node = if parent_path.is_file() {
                let parent = ArtifactManifest::load(&parent_path)?;
                ensure!(
                    parent.artifact_id.eq_ignore_ascii_case(&edge.parent_artifact_id),
                    "lineage parent manifest identity mismatch for {}",
                    edge.parent_artifact_id
                );
                build_node(store, &parent, depth + 1, max_depth, stack)?
            } else {
                LineageGraphNode {
                    artifact_id: edge.parent_artifact_id.clone(),
                    source_name: None,
                    format: None,
                    provenance: None,
                    parents: Vec::new(),
                    missing: true,
                    truncated: false,
                }
            };
            node.parents.push(LineageGraphEdge {
                operation: edge.operation.clone(),
                note: edge.note.clone(),
                parent: Box::new(parent_node),
            });
        }
    }

    stack.remove(&id);
    Ok(node)
}
