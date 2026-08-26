use std::fs;

use modelvault::cas::{CompressionMode, LocalCas};
use tempfile::tempdir;

#[test]
fn repository_metadata_is_created_and_persisted() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("store");
    let mut cas = LocalCas::open(&root).unwrap();
    assert_eq!(cas.metadata().version, 1);
    assert_eq!(cas.metadata().object_hash, "blake3");
    assert!(root.join("repository.json").is_file());

    cas.set_compression(CompressionMode::Zstd, 3).unwrap();
    let reopened = LocalCas::open(&root).unwrap();
    assert_eq!(reopened.metadata().loose_compression, CompressionMode::Zstd);
    assert_eq!(reopened.metadata().zstd_level, 3);
}

#[test]
fn zstd_migration_preserves_object_identity_and_bytes() {
    let dir = tempdir().unwrap();
    let mut cas = LocalCas::open(dir.path()).unwrap();
    let bytes = vec![b'A'; 512 * 1024];
    let put = cas.put_bytes(&bytes).unwrap();
    let raw_size = fs::metadata(cas.object_path(&put.id)).unwrap().len();

    let report = cas.migrate_loose_compression(CompressionMode::Zstd, 3).unwrap();
    assert_eq!(report.objects_rewritten, 1);
    assert!(report.after_bytes < raw_size);
    assert_eq!(cas.read(&put.id).unwrap(), bytes);
    assert!(cas.verify(&put.id).unwrap());

    let report = cas.migrate_loose_compression(CompressionMode::None, 3).unwrap();
    assert_eq!(report.objects_rewritten, 1);
    assert_eq!(cas.read(&put.id).unwrap(), bytes);
    assert!(cas.verify(&put.id).unwrap());
}

#[test]
fn repack_can_remove_loose_objects_without_breaking_reads() {
    let dir = tempdir().unwrap();
    let cas = LocalCas::open(dir.path()).unwrap();
    let one = cas.put_bytes(b"first packed object").unwrap();
    let two = cas.put_bytes(b"second packed object").unwrap();

    let report = cas.repack(true).unwrap();
    assert_eq!(report.objects_packed, 2);
    assert_eq!(report.loose_removed, 2);
    assert!(!cas.object_path(&one.id).exists());
    assert!(!cas.object_path(&two.id).exists());
    assert_eq!(cas.read(&one.id).unwrap(), b"first packed object");
    assert_eq!(cas.read(&two.id).unwrap(), b"second packed object");
    assert!(cas.verify(&one.id).unwrap());
    assert!(cas.verify(&two.id).unwrap());
}
