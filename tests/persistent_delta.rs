use std::fs;

use modelvault::{
    artifact::{add_raw_artifact, materialize},
    cas::{LocalCas, ObjectId},
    delta::optimize_delta_storage,
    repository::{fsck, gc},
};
use tempfile::tempdir;

#[test]
fn persistent_delta_round_trips_and_preserves_object_id() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let store = dir.path().join("store");
    let base_path = dir.path().join("base.bin");
    let target_path = dir.path().join("target.bin");
    let restored = dir.path().join("restored.bin");

    let base = vec![0x31u8; 64 * 1024];
    let mut target = base.clone();
    target[1234] ^= 0x7f;
    target[48_000] ^= 0x11;
    fs::write(&base_path, &base)?;
    fs::write(&target_path, &target)?;

    let cas = LocalCas::open(&store)?;
    let left = add_raw_artifact(&base_path, &cas, 64 * 1024)?.manifest;
    let right = add_raw_artifact(&target_path, &cas, 64 * 1024)?.manifest;
    let target_id = ObjectId::parse(&right.chunks[0].object)?;
    let original_id = target_id.clone();

    let report = optimize_delta_storage(&left, &right, &cas, 3, 20, 2)?;
    assert_eq!(report.stored, 1);
    assert!(!cas.object_path(&target_id).exists());
    assert!(cas.delta_path(&target_id).exists());
    assert_eq!(cas.delta_depth(&target_id)?, 1);
    assert!(cas.verify(&target_id)?);
    assert_eq!(ObjectId::from_bytes(&cas.read(&target_id)?), original_id);

    materialize(&right, &cas, &restored)?;
    assert_eq!(fs::read(restored)?, target);
    Ok(())
}

#[test]
fn delta_chains_are_bounded_by_policy() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let store = dir.path().join("store");
    let cas = LocalCas::open(&store)?;
    let mut bytes = vec![0x44u8; 64 * 1024];

    let mut manifests = Vec::new();
    for i in 0..4 {
        if i > 0 {
            bytes[1000 * i] ^= i as u8;
        }
        let path = dir.path().join(format!("v{i}.bin"));
        fs::write(&path, &bytes)?;
        manifests.push(add_raw_artifact(&path, &cas, 64 * 1024)?.manifest);
    }

    let first = optimize_delta_storage(&manifests[0], &manifests[1], &cas, 3, 20, 2)?;
    assert_eq!(first.stored, 1);
    let second = optimize_delta_storage(&manifests[1], &manifests[2], &cas, 3, 20, 2)?;
    assert_eq!(second.stored, 1);
    let third = optimize_delta_storage(&manifests[2], &manifests[3], &cas, 3, 20, 2)?;
    assert_eq!(third.stored, 0);
    assert_eq!(third.skipped, 1);

    let v2 = ObjectId::parse(&manifests[2].chunks[0].object)?;
    let v3 = ObjectId::parse(&manifests[3].chunks[0].object)?;
    assert_eq!(cas.delta_depth(&v2)?, 2);
    assert_eq!(cas.delta_depth(&v3)?, 0);
    assert!(cas.verify(&v2)?);
    assert!(cas.verify(&v3)?);
    Ok(())
}

#[test]
fn policy_skips_delta_when_savings_are_insufficient() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let store = dir.path().join("store");
    let a = dir.path().join("a.bin");
    let b = dir.path().join("b.bin");
    fn deterministic_bytes(mut state: u64, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.push((state >> 24) as u8);
        }
        out
    }

    // Two unrelated pseudo-random streams are intentionally hard to compress.
    // Their XOR is also hard to compress, so the delta record should not beat
    // the full object by the deliberately strict 95% savings threshold.
    let left_bytes = deterministic_bytes(0x1234_5678_9abc_def0, 64 * 1024);
    let right_bytes = deterministic_bytes(0xfedc_ba98_7654_3210, 64 * 1024);
    fs::write(&a, left_bytes)?;
    fs::write(&b, right_bytes)?;
    let cas = LocalCas::open(&store)?;
    let left = add_raw_artifact(&a, &cas, 64 * 1024)?.manifest;
    let right = add_raw_artifact(&b, &cas, 64 * 1024)?.manifest;
    let target = ObjectId::parse(&right.chunks[0].object)?;

    let report = optimize_delta_storage(&left, &right, &cas, 3, 95, 2)?;
    assert_eq!(report.stored, 0);
    assert!(cas.object_path(&target).is_file());
    assert!(!cas.delta_path(&target).exists());
    Ok(())
}

#[test]
fn gc_preserves_delta_base_dependencies() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let store = dir.path().join("store");
    let base_path = dir.path().join("base.bin");
    let target_path = dir.path().join("target.bin");
    let base = vec![0x52u8; 64 * 1024];
    let mut target = base.clone();
    target[777] ^= 1;
    fs::write(&base_path, &base)?;
    fs::write(&target_path, &target)?;
    let cas = LocalCas::open(&store)?;
    let left_result = add_raw_artifact(&base_path, &cas, 64 * 1024)?;
    let right_result = add_raw_artifact(&target_path, &cas, 64 * 1024)?;
    optimize_delta_storage(
        &left_result.manifest,
        &right_result.manifest,
        &cas,
        3,
        20,
        2,
    )?;

    // Remove the base manifest. The base object is still reachable through the target delta.
    fs::remove_file(left_result.manifest_path)?;
    let base_id = ObjectId::parse(&left_result.manifest.chunks[0].object)?;
    let dry = gc(&store, false)?;
    assert_eq!(dry.orphan_objects, 0);
    let pruned = gc(&store, true)?;
    assert_eq!(pruned.removed_objects, 0);
    assert!(cas.contains(&base_id));
    assert!(fsck(&store, true)?.is_ok());
    Ok(())
}

#[test]
fn repository_metadata_defaults_delta_policy_for_legacy_json() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let store = dir.path().join("store");
    fs::create_dir_all(&store)?;
    fs::write(
        store.join("repository.json"),
        r#"{"version":1,"object_hash":"blake3","loose_compression":"none","zstd_level":3,"pack_format_version":1}"#,
    )?;
    let cas = LocalCas::open(&store)?;
    assert_eq!(cas.metadata().delta_min_savings_pct, 20);
    assert_eq!(cas.metadata().max_delta_depth, 2);
    Ok(())
}
