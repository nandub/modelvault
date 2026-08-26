use std::fs;

use modelvault::{artifact::{add_raw_artifact, materialize}, cas::LocalCas};
use tempfile::tempdir;

#[test]
fn compacted_pack_verifies_and_materializes_without_loose_objects() {
    let temp = tempdir().unwrap();
    let store = temp.path().join(".modelvault");
    let cas = LocalCas::open(&store).unwrap();
    let source = temp.path().join("source.bin");
    let restored = temp.path().join("restored.bin");
    fs::write(&source, (0..65536u32).flat_map(|n| n.to_le_bytes()).collect::<Vec<_>>()).unwrap();

    let added = add_raw_artifact(&source, &cas, 4096).unwrap();
    let compact = cas.compact_packs(true, true).unwrap();
    assert!(compact.objects_packed > 0);
    assert!(compact.loose_removed > 0);

    let verified = cas.verify_packs().unwrap();
    assert!(verified.is_ok(), "{:?}", verified.errors);
    assert_eq!(verified.objects_verified, compact.objects_packed);

    materialize(&added.manifest, &cas, &restored).unwrap();
    assert_eq!(fs::read(&source).unwrap(), fs::read(&restored).unwrap());
}
