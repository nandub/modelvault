use std::fs;

use modelvault::{artifact::add_raw_artifact, cas::LocalCas, delta::analyze_delta_potential};
use tempfile::tempdir;

#[test]
fn delta_analysis_finds_savings_for_small_aligned_changes() {
    let temp = tempdir().unwrap();
    let store = temp.path().join(".modelvault");
    let cas = LocalCas::open(&store).unwrap();
    let left_path = temp.path().join("left.bin");
    let right_path = temp.path().join("right.bin");

    let mut state = 0x1234_5678u32;
    let left = (0..16 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect::<Vec<_>>();
    let mut right = left.clone();
    for byte in &mut right[4096..4160] {
        *byte ^= 0x01;
    }
    fs::write(&left_path, left).unwrap();
    fs::write(&right_path, right).unwrap();

    let left_manifest = add_raw_artifact(&left_path, &cas, 4096).unwrap().manifest;
    let right_manifest = add_raw_artifact(&right_path, &cas, 4096).unwrap().manifest;
    let report = analyze_delta_potential(&left_manifest, &right_manifest, &cas, 3).unwrap();

    assert_eq!(report.right_changed_chunks, 1);
    assert_eq!(report.comparable_chunks, 1);
    assert!(report.delta_compressed_bytes < report.full_compressed_bytes);
    assert!(report.potential_savings_bytes > 0);
}
