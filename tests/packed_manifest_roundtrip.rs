use std::fs;

use modelvault::{artifact::{add_raw_artifact, materialize}, cas::LocalCas};
use tempfile::tempdir;

#[test]
fn artifact_materializes_after_objects_are_packed_and_loose_removed() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("store");
    let source = dir.path().join("source.bin");
    let output = dir.path().join("restored.bin");
    let bytes = (0..(1024 * 256)).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    fs::write(&source, &bytes).unwrap();

    let cas = LocalCas::open(&store).unwrap();
    let result = add_raw_artifact(&source, &cas, 32 * 1024).unwrap();
    let packed = cas.repack(true).unwrap();
    assert!(packed.objects_packed > 0);

    materialize(&result.manifest, &cas, &output).unwrap();
    assert_eq!(fs::read(output).unwrap(), bytes);
}
