use std::path::Path;

use modelvault::{
    import::{huggingface_provenance, huggingface_snapshot_revision},
    manifest::{ArtifactManifest, ArtifactProvenance},
    pointer::ArtifactPointer,
};

#[test]
fn huggingface_provenance_uses_resolved_snapshot_without_local_cache_path() {
    let path = Path::new("C:/Users/test/.cache/huggingface/hub/models--org--demo/snapshots/abc123/model.safetensors");
    let provenance = huggingface_provenance("org/demo", Some("main"), path, "model.safetensors");

    assert_eq!(provenance.provider, "huggingface");
    assert_eq!(provenance.namespace.as_deref(), Some("org"));
    assert_eq!(provenance.model_name.as_deref(), Some("demo"));
    assert_eq!(provenance.repository.as_deref(), Some("org/demo"));
    assert_eq!(provenance.requested_revision.as_deref(), Some("main"));
    assert_eq!(provenance.resolved_revision.as_deref(), Some("abc123"));
    assert_eq!(
        provenance.source_uri.as_deref(),
        Some("hf://org/demo@abc123/model.safetensors")
    );
    assert!(!serde_json::to_string(&provenance).unwrap().contains("Users/test"));
}

#[test]
fn snapshot_revision_is_extracted_from_cache_path() {
    assert_eq!(
        huggingface_snapshot_revision(Path::new("hub/models--org--demo/snapshots/deadbeef/model.safetensors")).as_deref(),
        Some("deadbeef")
    );
}

#[test]
fn pointer_carries_manifest_provenance() {
    let provenance = ArtifactProvenance {
        provider: "huggingface".into(),
        namespace: Some("org".into()),
        repository: Some("org/demo".into()),
        model_name: Some("demo".into()),
        requested_revision: Some("main".into()),
        resolved_revision: Some("abc123".into()),
        filename: Some("model.safetensors".into()),
        source_uri: Some("hf://org/demo@abc123/model.safetensors".into()),
    };
    let manifest = ArtifactManifest {
        version: 1,
        artifact_id: "abc".into(),
        format: "safetensors".into(),
        source_name: "model.safetensors".into(),
        logical_size: 42,
        chunk_size: 4,
        provenance: Some(provenance.clone()),
        lineage: Vec::new(),
        chunks: vec![],
        tensors: vec![],
    };

    assert_eq!(ArtifactPointer::from_manifest(&manifest).provenance, Some(provenance));
}

#[test]
fn legacy_manifest_without_provenance_still_deserializes() {
    let json = r#"{
      "version": 1,
      "artifact_id": "abc",
      "format": "raw",
      "source_name": "artifact.bin",
      "logical_size": 0,
      "chunk_size": 4096,
      "chunks": []
    }"#;
    let manifest: ArtifactManifest = serde_json::from_str(json).unwrap();
    assert!(manifest.provenance.is_none());
}
