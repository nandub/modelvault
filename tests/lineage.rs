use std::fs;

use modelvault::{
    lineage::{add_lineage_edge, build_lineage_graph, ensure_no_lineage_cycle},
    manifest::{ArtifactManifest, ArtifactProvenance},
    pointer::ArtifactPointer,
};

fn id(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn empty_manifest(artifact_id: String, source_name: &str) -> ArtifactManifest {
    ArtifactManifest {
        version: 1,
        artifact_id,
        format: "raw".to_string(),
        source_name: source_name.to_string(),
        logical_size: 0,
        chunk_size: 4096,
        provenance: None,
        lineage: Vec::new(),
        chunks: Vec::new(),
        tensors: Vec::new(),
    }
}

#[test]
fn lineage_is_optional_for_legacy_manifests() {
    let json = format!(
        r#"{{"version":1,"artifact_id":"{}","format":"raw","source_name":"legacy.bin","logical_size":0,"chunk_size":4096,"chunks":[],"tensors":[]}}"#,
        id('a')
    );
    let manifest: ArtifactManifest = serde_json::from_str(&json).unwrap();
    assert!(manifest.lineage.is_empty());
}

#[test]
fn pointer_carries_lineage_when_present() {
    let parent = empty_manifest(id('a'), "base.bin");
    let mut child = empty_manifest(id('b'), "derived.bin");
    assert!(add_lineage_edge(&mut child, &parent, "fine-tune", Some("training run 42")).unwrap());
    let pointer = ArtifactPointer::from_manifest(&child);
    assert_eq!(pointer.lineage, child.lineage);
}

#[test]
fn graph_traverses_multiple_generations() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".modelvault");
    fs::create_dir_all(store.join("manifests")).unwrap();

    let base = empty_manifest(id('a'), "base.bin");
    base.save(&store).unwrap();

    let mut tuned = empty_manifest(id('b'), "tuned.bin");
    add_lineage_edge(&mut tuned, &base, "fine-tune", None).unwrap();
    tuned.save(&store).unwrap();

    let mut quantized = empty_manifest(id('c'), "quantized.bin");
    add_lineage_edge(&mut quantized, &tuned, "quantize", None).unwrap();
    let graph = build_lineage_graph(&store, &quantized, 16).unwrap();

    assert_eq!(graph.parents[0].operation, "quantize");
    assert_eq!(graph.parents[0].parent.parents[0].operation, "fine-tune");
    assert_eq!(
        graph.parents[0].parent.parents[0].parent.artifact_id,
        base.artifact_id
    );
}

#[test]
fn graph_preserves_missing_parent_edge() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".modelvault");
    fs::create_dir_all(store.join("manifests")).unwrap();

    let parent = empty_manifest(id('a'), "base.bin");
    let mut child = empty_manifest(id('b'), "derived.bin");
    add_lineage_edge(&mut child, &parent, "convert", None).unwrap();

    let graph = build_lineage_graph(&store, &child, 16).unwrap();
    assert!(graph.parents[0].parent.missing);
    assert_eq!(graph.parents[0].parent.artifact_id, parent.artifact_id);
}

#[test]
fn cycle_creation_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".modelvault");
    fs::create_dir_all(store.join("manifests")).unwrap();

    let base = empty_manifest(id('a'), "base.bin");
    let mut child = empty_manifest(id('b'), "child.bin");
    add_lineage_edge(&mut child, &base, "fine-tune", None).unwrap();
    child.save(&store).unwrap();

    let err = ensure_no_lineage_cycle(&store, &child, &base.artifact_id).unwrap_err();
    assert!(err.to_string().contains("cycle"));
}

#[test]
fn lineage_metadata_does_not_require_provenance() {
    let parent = empty_manifest(id('a'), "base.bin");
    let mut child = empty_manifest(id('b'), "child.bin");
    child.provenance = Some(ArtifactProvenance {
        provider: "test".into(),
        namespace: None,
        repository: None,
        model_name: None,
        requested_revision: None,
        resolved_revision: None,
        filename: None,
        source_uri: None,
    });
    add_lineage_edge(&mut child, &parent, "distill", None).unwrap();
    assert_eq!(child.lineage.len(), 1);
}
