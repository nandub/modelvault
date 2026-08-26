use std::fs;

use modelvault::{
    artifact::materialize,
    cas::{LocalCas, ObjectId},
    manifest::ArtifactManifest,
    pointer::ArtifactPointer,
};
use tempfile::tempdir;

fn empty_manifest(id: &str) -> ArtifactManifest {
    ArtifactManifest {
        version: 1,
        artifact_id: id.to_string(),
        format: "raw".to_string(),
        source_name: "artifact.bin".to_string(),
        logical_size: 0,
        chunk_size: 4096,
        provenance: None,
        lineage: Vec::new(),
        chunks: Vec::new(),
        tensors: Vec::new(),
    }
}

#[test]
fn pointer_rejects_manifest_path_traversal() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let id = "a".repeat(64);
    let mut pointer = ArtifactPointer::from_manifest(&empty_manifest(&id));
    pointer.manifest = "../../outside.json".to_string();

    let err = pointer.resolve_manifest(repo.path()).unwrap_err();
    assert!(err.to_string().contains("pointer manifest must be"));
    Ok(())
}

#[test]
fn pointer_rejects_manifest_identity_mismatch() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let id = "b".repeat(64);
    let store = repo.path().join(".modelvault");
    let manifest = empty_manifest(&id);
    manifest.save(&store)?;

    let mut pointer = ArtifactPointer::from_manifest(&manifest);
    pointer.logical_size = 1;
    let err = pointer.resolve_manifest(repo.path()).unwrap_err();
    assert!(err.to_string().contains("logical size"));
    Ok(())
}

#[test]
fn malformed_manifest_is_rejected_before_materialization_creates_output() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let cas = LocalCas::open(repo.path().join(".modelvault"))?;
    let mut manifest = empty_manifest(&"c".repeat(64));
    manifest.logical_size = u64::MAX;
    let output = repo.path().join("nested").join("artifact.bin");

    let err = materialize(&manifest, &cas, &output).unwrap_err();
    assert!(err.to_string().contains("manifest covers"));
    assert!(!output.exists());
    assert!(!output.parent().expect("parent").exists());
    Ok(())
}

#[cfg(feature = "compression")]
#[test]
fn compressed_object_decoder_stops_at_declared_logical_size() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let cas = LocalCas::open(repo.path().join(".modelvault"))?;
    let id = ObjectId::parse(&"d".repeat(64))?;
    let path = cas.object_path(&id);
    fs::create_dir_all(path.parent().expect("object parent"))?;

    let payload = vec![0u8; 1024 * 1024];
    let compressed = zstd::stream::encode_all(std::io::Cursor::new(payload), 3)?;
    let mut encoded = Vec::with_capacity(12 + compressed.len());
    encoded.extend_from_slice(b"MVZ1");
    encoded.extend_from_slice(&10u64.to_le_bytes());
    encoded.extend_from_slice(&compressed);
    fs::write(path, encoded)?;

    let err = cas.read(&id).unwrap_err();
    assert!(err.to_string().contains("decoded to 11 bytes, expected 10"));
    Ok(())
}

#[test]
fn pack_index_rejects_pack_file_traversal() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let store = repo.path().join(".modelvault");
    let cas = LocalCas::open(&store)?;
    let id = ObjectId::parse(&"e".repeat(64))?;
    let packs = store.join("packs");
    fs::create_dir_all(&packs)?;
    fs::write(
        packs.join("malicious.idx.json"),
        format!(
            r#"{{"version":2,"pack_file":"../../outside.mvpack","objects":[{{"id":"{}","offset":0,"size":1,"stored_size":1,"encoding":"raw"}}]}}"#,
            id
        ),
    )?;

    let err = cas.read(&id).unwrap_err();
    assert!(err.to_string().contains("invalid pack_file path"));
    Ok(())
}
