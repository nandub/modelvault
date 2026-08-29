use std::fs;

use modelvault::{artifact::add_raw_artifact, cas::LocalCas, repository::analytics_report};
use tempfile::tempdir;

#[test]
fn analytics_reports_cross_artifact_reuse() {
    let temp = tempdir().unwrap();
    let store = temp.path().join(".modelvault");
    let cas = LocalCas::open(&store).unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");

    // Build two distinct artifacts with seven shared 4 KiB chunks and one
    // different final chunk. Using identical files would intentionally produce
    // the same artifact ID and therefore a single content-addressed manifest.
    let mut a_bytes = Vec::with_capacity(8 * 4096);
    let mut b_bytes = Vec::with_capacity(8 * 4096);
    for value in 0u8..8 {
        a_bytes.extend(std::iter::repeat_n(value, 4096));
        let b_value = if value == 7 { 0xFF } else { value };
        b_bytes.extend(std::iter::repeat_n(b_value, 4096));
    }

    fs::write(&a, &a_bytes).unwrap();
    fs::write(&b, &b_bytes).unwrap();
    add_raw_artifact(&a, &cas, 4096).unwrap();
    add_raw_artifact(&b, &cas, 4096).unwrap();

    let report = analytics_report(&store).unwrap();
    assert_eq!(report.artifacts.len(), 2);
    assert_eq!(
        report.total_logical_bytes,
        (a_bytes.len() + b_bytes.len()) as u64
    );
    assert!(report.dedup_savings_bytes >= 7 * 4096);
    assert!(report
        .artifacts
        .iter()
        .any(|artifact| artifact.shared_bytes >= 7 * 4096));
}

#[test]
fn identical_content_resolves_to_one_logical_artifact_manifest() {
    let temp = tempdir().unwrap();
    let store = temp.path().join(".modelvault");
    let cas = LocalCas::open(&store).unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");
    let bytes = vec![0x31u8; 32 * 1024];

    fs::write(&a, &bytes).unwrap();
    fs::write(&b, &bytes).unwrap();
    let first = add_raw_artifact(&a, &cas, 4096).unwrap();
    let second = add_raw_artifact(&b, &cas, 4096).unwrap();

    assert_eq!(first.manifest.artifact_id, second.manifest.artifact_id);

    let report = analytics_report(&store).unwrap();
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(report.total_logical_bytes, bytes.len() as u64);
}
