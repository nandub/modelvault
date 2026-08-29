use std::fs;

use modelvault::{
    artifact::{add_raw_artifact, materialize},
    cas::{CompressionMode, LocalCas},
    repository::storage_report,
};
use tempfile::tempdir;

#[test]
fn storage_distinguishes_duplicate_representations_from_orphans() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let store = temp.path().join("store");
    fs::write(&source, vec![b'A'; 128 * 1024]).unwrap();
    let mut cas = LocalCas::open(&store).unwrap();
    add_raw_artifact(&source, &cas, 4096).unwrap();
    cas.migrate_loose_compression(CompressionMode::Zstd, 3)
        .unwrap();
    cas.repack(false).unwrap();

    let report = storage_report(&store).unwrap();
    assert_eq!(report.orphan_objects, 0);
    assert_eq!(report.orphan_bytes, 0);
    assert!(report.duplicate_representation_bytes > 0);
    assert!(report.loose_compressed_bytes > 0);
    assert!(report.pack_data_bytes > 0);
}

#[test]
fn optimize_creates_verified_pack_v2_and_removes_redundant_loose_objects() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let restored = temp.path().join("restored.bin");
    let store = temp.path().join("store");
    let bytes = vec![b'Z'; 256 * 1024];
    fs::write(&source, &bytes).unwrap();
    let cas = LocalCas::open(&store).unwrap();
    let added = add_raw_artifact(&source, &cas, 4096).unwrap();

    let dry = cas.optimize_representations(true).unwrap();
    assert!(dry.dry_run);
    assert!(dry.objects_considered > 0);

    let applied = cas.optimize_representations(false).unwrap();
    assert!(!applied.dry_run);
    assert!(cas.verify_packs().unwrap().is_ok());
    for chunk in &added.manifest.chunks {
        let id = modelvault::cas::ObjectId::parse(&chunk.object).unwrap();
        assert!(cas.verify(&id).unwrap());
        assert!(!cas.object_path(&id).exists());
    }
    materialize(&added.manifest, &cas, &restored).unwrap();
    assert_eq!(fs::read(restored).unwrap(), bytes);
}
