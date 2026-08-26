use std::fs;

use modelvault::{
    artifact::{add_raw_artifact, materialize},
    cas::LocalCas,
    remote::{pull_manifest, push_manifest},
};
use tempfile::tempdir;

#[test]
fn filesystem_remote_push_pull_round_trips() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let local_root = temp.path().join("local");
    let remote_root = temp.path().join("remote");
    let restored_root = temp.path().join("restored-store");
    let output = temp.path().join("restored.bin");

    let bytes: Vec<u8> = (0..65_537).map(|i| (i % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();

    let local = LocalCas::open(&local_root).unwrap();
    let added = add_raw_artifact(&source, &local, 4096).unwrap();

    let pushed = push_manifest(&added.manifest, &local, &remote_root).unwrap();
    assert!(pushed.objects_copied > 0);
    assert!(pushed.elapsed_ms < u128::MAX);

    let restored = LocalCas::open(&restored_root).unwrap();
    let pulled = pull_manifest(&added.manifest, &restored, &remote_root).unwrap();
    assert!(pulled.objects_copied > 0);

    materialize(&added.manifest, &restored, &output).unwrap();
    assert_eq!(fs::read(output).unwrap(), bytes);
}

#[test]
fn second_push_reuses_remote_objects() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("artifact.bin");
    let local_root = temp.path().join("local");
    let remote_root = temp.path().join("remote");
    fs::write(&source, vec![7u8; 20_000]).unwrap();

    let local = LocalCas::open(&local_root).unwrap();
    let added = add_raw_artifact(&source, &local, 4096).unwrap();
    let first = push_manifest(&added.manifest, &local, &remote_root).unwrap();
    let second = push_manifest(&added.manifest, &local, &remote_root).unwrap();

    assert!(first.objects_copied > 0);
    assert_eq!(second.objects_copied, 0);
    assert_eq!(second.bytes_copied, 0);
    assert_eq!(second.bytes_reused, added.manifest.logical_size);
}

#[test]
fn parallel_push_is_restartable_at_object_granularity() {
    use modelvault::remote::{push_manifest_with_options, SyncOptions};

    let temp = tempdir().unwrap();
    let source = temp.path().join("parallel.bin");
    let local_root = temp.path().join("local-parallel");
    let remote_root = temp.path().join("remote-parallel");
    let bytes: Vec<u8> = (0..200_000).map(|i| ((i * 17) % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();

    let local = LocalCas::open(&local_root).unwrap();
    let added = add_raw_artifact(&source, &local, 4096).unwrap();

    let first = push_manifest_with_options(
        &added.manifest,
        &local,
        &remote_root,
        &SyncOptions { jobs: 4, deep_verify: false },
    ).unwrap();
    assert!(first.objects_copied > 0);

    let second = push_manifest_with_options(
        &added.manifest,
        &local,
        &remote_root,
        &SyncOptions { jobs: 4, deep_verify: false },
    ).unwrap();
    assert_eq!(second.objects_copied, 0);
    assert_eq!(second.objects_reused, second.objects_total);
    assert_eq!(second.bytes_reused, added.manifest.logical_size);
}
