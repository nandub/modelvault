use std::fs;

use modelvault::{
    artifact::add_raw_artifact,
    cas::LocalCas,
    repository::{analytics_report, benchmark_snapshot, storage_efficiency_report},
};
use tempfile::tempdir;

#[test]
fn efficiency_decomposes_dedup_and_compression() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    let cas = LocalCas::open(&store).unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");

    let shared = vec![b'A'; 4096];
    let mut a_bytes = Vec::new();
    let mut b_bytes = Vec::new();
    for _ in 0..7 {
        a_bytes.extend_from_slice(&shared);
        b_bytes.extend_from_slice(&shared);
    }
    a_bytes.extend(vec![b'B'; 4096]);
    b_bytes.extend(vec![b'C'; 4096]);
    fs::write(&a, &a_bytes).unwrap();
    fs::write(&b, &b_bytes).unwrap();
    add_raw_artifact(&a, &cas, 4096).unwrap();
    add_raw_artifact(&b, &cas, 4096).unwrap();

    let report = storage_efficiency_report(&store).unwrap();
    assert_eq!(report.logical_bytes, (a_bytes.len() + b_bytes.len()) as u64);
    assert!(report.dedup_savings_bytes >= 7 * 4096);
    assert!(report.compression_savings_bytes > 0);
    assert!(report.full_encoded_bytes <= report.unique_logical_bytes);
    assert!(report.primary_encoded_bytes <= report.full_encoded_bytes);
}

#[test]
fn artifact_attribution_is_order_independent_and_shared() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    let cas = LocalCas::open(&store).unwrap();
    let a = temp.path().join("a.bin");
    let b = temp.path().join("b.bin");
    let mut a_bytes = vec![1u8; 4096];
    a_bytes.extend(vec![2u8; 4096]);
    let mut b_bytes = vec![1u8; 4096];
    b_bytes.extend(vec![3u8; 4096]);
    fs::write(&a, &a_bytes).unwrap();
    fs::write(&b, &b_bytes).unwrap();
    add_raw_artifact(&a, &cas, 4096).unwrap();
    add_raw_artifact(&b, &cas, 4096).unwrap();

    let report = analytics_report(&store).unwrap();
    assert_eq!(report.artifacts.len(), 2);
    for artifact in &report.artifacts {
        assert_eq!(artifact.shared_bytes, 4096);
        assert_eq!(artifact.exclusive_bytes, 4096);
        assert!(artifact.attributed_physical_bytes > 0);
    }
}

#[test]
fn benchmark_snapshot_serializes_for_comparison() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    let cas = LocalCas::open(&store).unwrap();
    let source = temp.path().join("model.bin");
    fs::write(&source, vec![b'Z'; 16 * 1024]).unwrap();
    add_raw_artifact(&source, &cas, 4096).unwrap();

    let snapshot = benchmark_snapshot(&store).unwrap();
    assert_eq!(snapshot.format_version, 1);
    assert_eq!(snapshot.manifests, 1);
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(json.contains("dedup_savings_pct"));
    assert!(json.contains("compression_savings_pct"));
}
