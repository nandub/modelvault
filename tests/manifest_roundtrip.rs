use std::fs;

use modelvault::{
    artifact::{
        add_raw_artifact, add_raw_artifact_with_progress, materialize, materialize_with_progress,
        verify_artifact, ArtifactProgressPhase,
    },
    cas::LocalCas,
    manifest::ArtifactManifest,
};
use tempfile::tempdir;

#[test]
fn raw_artifact_round_trips_byte_for_byte() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source.bin");
    let output = temp.path().join("restored.bin");
    let bytes: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();
    let cas = LocalCas::open(temp.path().join(".modelvault")).unwrap();
    let added = add_raw_artifact(&source, &cas, 4096).unwrap();
    verify_artifact(&added.manifest, &cas).unwrap();
    materialize(&added.manifest, &cas, &output).unwrap();
    assert_eq!(fs::read(source).unwrap(), fs::read(output).unwrap());

    let loaded = ArtifactManifest::load(added.manifest_path).unwrap();
    assert_eq!(loaded.artifact_id, added.manifest.artifact_id);
}

#[test]
fn second_identical_artifact_reuses_every_chunk() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source.bin");
    fs::write(&source, vec![42u8; 20_000]).unwrap();
    let cas = LocalCas::open(temp.path().join(".modelvault")).unwrap();
    let first = add_raw_artifact(&source, &cas, 4096).unwrap();
    let second = add_raw_artifact(&source, &cas, 4096).unwrap();
    assert!(first.new_bytes > 0);
    assert_eq!(second.new_bytes, 0);
    assert_eq!(second.reused_bytes, second.manifest.logical_size);
}

#[test]
fn ingest_and_materialize_report_completed_progress() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source.bin");
    let output = temp.path().join("restored.bin");
    let bytes = vec![7u8; 10_000];
    fs::write(&source, &bytes).unwrap();
    let cas = LocalCas::open(temp.path().join(".modelvault")).unwrap();

    let mut ingest = Vec::new();
    let added = add_raw_artifact_with_progress(&source, &cas, 4096, &mut |phase, done, total| {
        ingest.push((phase, done, total));
    })
    .unwrap();
    assert!(ingest.contains(&(ArtifactProgressPhase::Hashing, 10_000, 10_000)));
    assert!(ingest.contains(&(ArtifactProgressPhase::Storing, 10_000, 10_000)));

    let mut restored = Vec::new();
    materialize_with_progress(&added.manifest, &cas, &output, &mut |phase, done, total| {
        restored.push((phase, done, total));
    })
    .unwrap();
    assert!(restored.contains(&(ArtifactProgressPhase::Materializing, 10_000, 10_000)));
    assert!(restored.contains(&(ArtifactProgressPhase::Verifying, 10_000, 10_000)));
}
