use modelvault::{manifest::ArtifactManifest, pointer::ArtifactPointer};

#[test]
fn pointer_round_trips() {
    let manifest = ArtifactManifest { version:1, artifact_id:"abc123".into(), format:"safetensors".into(), source_name:"model.safetensors".into(), logical_size:42, chunk_size:4, provenance:None, lineage:vec![], chunks:vec![], tensors:vec![] };
    let pointer = ArtifactPointer::from_manifest(&manifest);
    let dir = tempfile::tempdir().unwrap(); let path=dir.path().join("model.safetensors.mvptr"); pointer.save(&path).unwrap();
    assert_eq!(pointer, ArtifactPointer::load(&path).unwrap());
    assert_eq!(pointer.manifest, ".modelvault/manifests/abc123.json");
}
