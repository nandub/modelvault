use std::fs;

use modelvault::{
    artifact::add_raw_artifact,
    cas::{CompressionMode, LocalCas},
    repository::{fsck, gc, storage_report},
};
use tempfile::tempdir;

#[test]
fn fsck_and_storage_report_healthy_repository() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let store = temp.path().join("store");
    fs::write(&source, vec![3u8; 10_000]).unwrap();

    let cas = LocalCas::open(&store).unwrap();
    let added = add_raw_artifact(&source, &cas, 1024).unwrap();

    let check = fsck(&store, true).unwrap();
    assert!(check.is_ok());
    assert_eq!(check.manifests_scanned, 1);
    assert_eq!(check.manifests_ok, 1);

    let report = storage_report(&store).unwrap();
    assert_eq!(report.manifests, 1);
    assert_eq!(report.logical_bytes, added.manifest.logical_size);
    assert!(report.physical_bytes <= report.logical_bytes);
    assert_eq!(report.orphan_objects, 0);
}

#[test]
fn gc_is_dry_run_until_prune_is_requested() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    let cas = LocalCas::open(&store).unwrap();
    let orphan = cas.put_bytes(b"unreferenced object").unwrap();

    let dry = gc(&store, false).unwrap();
    assert_eq!(dry.orphan_objects, 1);
    assert_eq!(dry.removed_objects, 0);
    assert!(cas.contains(&orphan.id));

    let pruned = gc(&store, true).unwrap();
    assert_eq!(pruned.removed_objects, 1);
    assert!(!cas.contains(&orphan.id));
}

#[test]
fn fsck_reports_missing_object() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let store = temp.path().join("store");
    fs::write(&source, vec![9u8; 4097]).unwrap();

    let cas = LocalCas::open(&store).unwrap();
    let added = add_raw_artifact(&source, &cas, 1024).unwrap();
    let id = modelvault::cas::ObjectId::parse(&added.manifest.chunks[0].object).unwrap();
    fs::remove_file(cas.object_path(&id)).unwrap();

    let check = fsck(&store, false).unwrap();
    assert!(!check.is_ok());
    assert_eq!(check.missing_objects, 1);
}

#[test]
fn deep_fsck_accepts_zstd_compressed_loose_objects() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let store = temp.path().join("store");
    fs::write(&source, vec![b'A'; 128 * 1024]).unwrap();

    let mut cas = LocalCas::open(&store).unwrap();
    add_raw_artifact(&source, &cas, 4096).unwrap();
    cas.migrate_loose_compression(CompressionMode::Zstd, 3)
        .unwrap();

    let check = fsck(&store, true).unwrap();
    assert!(
        check.is_ok(),
        "deep fsck errors: {:?}",
        check.manifest_errors
    );
    assert_eq!(check.manifests_ok, 1);
    assert_eq!(check.missing_objects, 0);
    assert_eq!(check.corrupt_objects, 0);
}
