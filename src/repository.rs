use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::{
    artifact::verify_artifact,
    cas::{LocalCas, ObjectId},
    manifest::{validate_manifest_structure, ArtifactManifest},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsckReport {
    pub manifests_scanned: usize,
    pub manifests_ok: usize,
    pub manifest_errors: Vec<String>,
    pub referenced_objects: usize,
    pub missing_objects: usize,
    pub corrupt_objects: usize,
    pub orphan_objects: usize,
}

impl FsckReport {
    pub fn is_ok(&self) -> bool {
        self.manifest_errors.is_empty() && self.missing_objects == 0 && self.corrupt_objects == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageReport {
    pub manifests: usize,
    pub logical_bytes: u64,
    pub object_count: usize,
    pub physical_bytes: u64,
    pub reachable_objects: usize,
    pub reachable_bytes: u64,
    pub orphan_objects: usize,
    pub orphan_bytes: u64,
    pub duplicate_representation_bytes: u64,
    pub loose_raw_bytes: u64,
    pub loose_compressed_bytes: u64,
    pub delta_bytes: u64,
    pub pack_data_bytes: u64,
    pub pack_index_bytes: u64,
    pub manifest_bytes: u64,
    pub metadata_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub orphan_objects: usize,
    pub orphan_bytes: u64,
    pub removed_objects: usize,
    pub removed_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageEfficiency {
    pub logical_bytes: u64,
    pub unique_logical_bytes: u64,
    pub dedup_savings_bytes: u64,
    pub dedup_savings_pct: f64,
    pub full_encoded_bytes: u64,
    pub compression_savings_bytes: u64,
    pub compression_savings_pct: f64,
    pub primary_encoded_bytes: u64,
    pub delta_savings_bytes: u64,
    pub delta_savings_pct: f64,
    pub duplicate_representation_bytes: u64,
    pub metadata_overhead_bytes: u64,
    pub actual_physical_bytes: u64,
    pub net_physical_savings_bytes: i128,
    pub net_physical_savings_pct: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtifactAnalytics {
    pub source_name: String,
    pub logical_bytes: u64,
    /// Bytes whose logical objects are referenced by exactly one artifact.
    pub exclusive_bytes: u64,
    /// Bytes whose logical objects are referenced by two or more artifacts.
    pub shared_bytes: u64,
    /// Approximate physical object bytes attributed to this artifact. Shared
    /// objects are divided evenly across artifacts that reference them.
    pub attributed_physical_bytes: u64,
    pub shared_pct: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RepositoryAnalytics {
    pub artifacts: Vec<ArtifactAnalytics>,
    pub total_logical_bytes: u64,
    pub unique_logical_object_bytes: u64,
    pub physical_bytes: u64,
    pub dedup_savings_bytes: u64,
    pub dedup_savings_pct: f64,
    pub physical_ratio: f64,
    pub efficiency: StorageEfficiency,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepositoryBenchmarkSnapshot {
    pub format_version: u32,
    pub manifests: usize,
    pub objects: usize,
    pub efficiency: StorageEfficiency,
    pub artifacts: Vec<ArtifactAnalytics>,
}

pub fn load_manifests(store_root: &Path) -> anyhow::Result<Vec<(PathBuf, ArtifactManifest)>> {
    let dir = store_root.join("manifests");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|v| v.to_str()).is_some_and(|v| v.eq_ignore_ascii_case("json")) {
            paths.push(path);
        }
    }
    paths.sort();

    let mut result = Vec::with_capacity(paths.len());
    for path in paths {
        let manifest = ArtifactManifest::load(&path)
            .with_context(|| format!("failed to load manifest {}", path.display()))?;
        result.push((path, manifest));
    }
    Ok(result)
}

fn referenced_object_ids(manifests: &[(PathBuf, ArtifactManifest)]) -> HashSet<String> {
    manifests.iter()
        .flat_map(|(_, m)| m.chunks.iter().map(|c| c.object.clone()))
        .collect()
}

fn referenced_object_ids_with_delta_dependencies(
    cas: &LocalCas,
    manifests: &[(PathBuf, ArtifactManifest)],
) -> anyhow::Result<HashSet<String>> {
    let mut refs = referenced_object_ids(manifests);
    let mut pending = refs.iter().cloned().collect::<Vec<_>>();
    while let Some(value) = pending.pop() {
        let id = ObjectId::parse(&value)?;
        if let Some(base) = cas.delta_base(&id)? {
            if refs.insert(base.to_string()) {
                pending.push(base.to_string());
            }
        }
    }
    Ok(refs)
}

pub fn fsck(store_root: &Path, deep: bool) -> anyhow::Result<FsckReport> {
    let cas = LocalCas::open(store_root)?;
    let mut report = FsckReport::default();
    let manifest_dir = store_root.join("manifests");
    let mut valid_manifests = Vec::new();

    if manifest_dir.exists() {
        let mut paths = fs::read_dir(&manifest_dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|v| v.to_str()).is_some_and(|v| v.eq_ignore_ascii_case("json")))
            .collect::<Vec<_>>();
        paths.sort();

        for path in paths {
            report.manifests_scanned += 1;
            match ArtifactManifest::load(&path) {
                Ok(manifest) => {
                    match validate_manifest_structure(&manifest) {
                        Ok(()) => {
                            if deep {
                                match verify_artifact(&manifest, &cas) {
                                    Ok(()) => report.manifests_ok += 1,
                                    Err(err) => report.manifest_errors.push(format!("{}: {err}", path.display())),
                                }
                            } else {
                                report.manifests_ok += 1;
                            }
                            valid_manifests.push((path, manifest));
                        }
                        Err(err) => report.manifest_errors.push(format!("{}: {err}", path.display())),
                    }
                }
                Err(err) => report.manifest_errors.push(format!("{}: {err}", path.display())),
            }
        }
    }

    let refs = referenced_object_ids_with_delta_dependencies(&cas, &valid_manifests)?;
    report.referenced_objects = refs.len();
    if !deep {
        for object in &refs {
            let id = ObjectId::parse(object)?;
            if !cas.contains(&id) {
                report.missing_objects += 1;
            }
        }
    } else {
        // Deep verify errors may already be summarized per manifest; these
        // counters provide object-level totals without double-counting refs.
        for object in &refs {
            let id = ObjectId::parse(object)?;
            if !cas.contains(&id) {
                report.missing_objects += 1;
            } else if !cas.verify(&id).unwrap_or(false) {
                report.corrupt_objects += 1;
            }
        }
    }

    let all = cas.list_objects()?;
    report.orphan_objects = all.iter().filter(|(id, _)| !refs.contains(id.as_str())).count();
    Ok(report)
}

pub fn storage_report(store_root: &Path) -> anyhow::Result<StorageReport> {
    let cas = LocalCas::open(store_root)?;
    let manifests = load_manifests(store_root)?;
    let refs = referenced_object_ids_with_delta_dependencies(&cas, &manifests)?;
    let objects = cas.list_objects()?;
    let mut logical_sizes: HashMap<String,u64> = HashMap::new();
    for (_,m) in &manifests { for c in &m.chunks { logical_sizes.entry(c.object.clone()).or_insert(c.size); } }
    for id in &refs { if !logical_sizes.contains_key(id) { let oid=ObjectId::parse(id)?; logical_sizes.insert(id.clone(), cas.read(&oid)?.len() as u64); } }
    let reachable_bytes = refs.iter().filter_map(|id| logical_sizes.get(id)).copied().sum::<u64>();
    let reachable_objects = refs.iter().filter(|id| cas.contains(&ObjectId::parse(id).expect("validated object id"))).count();
    let orphan_ids = objects.iter().filter(|(id,_)| !refs.contains(id.as_str())).map(|(id,_)|id.clone()).collect::<Vec<_>>();
    let mut orphan_bytes=0u64; for id in &orphan_ids { orphan_bytes=orphan_bytes.saturating_add(cas.representation_sizes(id)?.iter().sum::<u64>()); }
    let mut duplicate_representation_bytes=0u64;
    for id in &refs { let oid=ObjectId::parse(id)?; let mut reps=cas.representation_sizes(&oid)?; if reps.len()>1 { reps.sort_unstable(); duplicate_representation_bytes=duplicate_representation_bytes.saturating_add(reps.iter().skip(1).sum::<u64>()); } }
    let b=cas.physical_storage_breakdown()?;
    Ok(StorageReport { manifests:manifests.len(), logical_bytes:manifests.iter().map(|(_,m)|m.logical_size).sum(), object_count:objects.len(), physical_bytes:b.total(), reachable_objects, reachable_bytes, orphan_objects:orphan_ids.len(), orphan_bytes, duplicate_representation_bytes, loose_raw_bytes:b.loose_raw_bytes, loose_compressed_bytes:b.loose_compressed_bytes, delta_bytes:b.delta_bytes, pack_data_bytes:b.pack_data_bytes, pack_index_bytes:b.pack_index_bytes, manifest_bytes:b.manifest_bytes, metadata_bytes:b.metadata_bytes })
}

pub fn gc(store_root: &Path, prune: bool) -> anyhow::Result<GcReport> {
    let cas = LocalCas::open(store_root)?;
    let manifests = load_manifests(store_root)?;
    let refs = referenced_object_ids_with_delta_dependencies(&cas, &manifests)?;
    let mut report = GcReport::default();

    for (id, size) in cas.list_objects()? {
        if refs.contains(id.as_str()) {
            continue;
        }
        report.orphan_objects += 1;
        report.orphan_bytes = report.orphan_bytes.saturating_add(size);
        if prune && cas.remove_unpacked_object(&id)? {
            report.removed_objects += 1;
            report.removed_bytes = report.removed_bytes.saturating_add(size);
        }
    }
    Ok(report)
}

pub fn storage_efficiency_report(store_root: &Path) -> anyhow::Result<StorageEfficiency> {
    let cas = LocalCas::open(store_root)?;
    let manifests = load_manifests(store_root)?;
    let manifest_refs = referenced_object_ids(&manifests);
    let refs = referenced_object_ids_with_delta_dependencies(&cas, &manifests)?;

    let logical_bytes = manifests.iter().map(|(_, m)| m.logical_size).sum::<u64>();
    let mut logical_sizes = HashMap::<String, u64>::new();
    for (_, manifest) in &manifests {
        for chunk in &manifest.chunks {
            logical_sizes.entry(chunk.object.clone()).or_insert(chunk.size);
        }
    }
    for id in &refs {
        if !logical_sizes.contains_key(id) {
            let oid = ObjectId::parse(id)?;
            logical_sizes.insert(id.clone(), cas.read(&oid)?.len() as u64);
        }
    }

    let unique_logical_bytes = manifest_refs.iter().filter_map(|id| logical_sizes.get(id)).copied().sum::<u64>();
    let dedup_savings_bytes = logical_bytes.saturating_sub(unique_logical_bytes);
    let dedup_savings_pct = pct(dedup_savings_bytes, logical_bytes);

    let mut full_encoded_bytes = 0u64;
    let mut primary_encoded_bytes = 0u64;
    for id in &manifest_refs {
        let oid = ObjectId::parse(id)?;
        full_encoded_bytes = full_encoded_bytes.saturating_add(cas.estimated_full_encoded_size(&oid)?);
        primary_encoded_bytes = primary_encoded_bytes.saturating_add(cas.estimated_primary_physical_size(&oid)?);
    }
    // Delta bases that are not themselves referenced by a manifest are real
    // physical dependencies and must be included when evaluating delta benefit.
    for id in refs.difference(&manifest_refs) {
        let oid = ObjectId::parse(id)?;
        primary_encoded_bytes = primary_encoded_bytes.saturating_add(cas.estimated_primary_physical_size(&oid)?);
    }
    let compression_savings_bytes = unique_logical_bytes.saturating_sub(full_encoded_bytes);
    let compression_savings_pct = pct(compression_savings_bytes, unique_logical_bytes);
    let delta_savings_bytes = full_encoded_bytes.saturating_sub(primary_encoded_bytes);
    let delta_savings_pct = pct(delta_savings_bytes, full_encoded_bytes);

    let storage = storage_report(store_root)?;
    let metadata_overhead_bytes = storage.pack_index_bytes
        .saturating_add(storage.manifest_bytes)
        .saturating_add(storage.metadata_bytes);
    let net_physical_savings_bytes = logical_bytes as i128 - storage.physical_bytes as i128;
    let net_physical_savings_pct = if logical_bytes == 0 {
        0.0
    } else {
        net_physical_savings_bytes as f64 / logical_bytes as f64 * 100.0
    };

    Ok(StorageEfficiency {
        logical_bytes,
        unique_logical_bytes,
        dedup_savings_bytes,
        dedup_savings_pct,
        full_encoded_bytes,
        compression_savings_bytes,
        compression_savings_pct,
        primary_encoded_bytes,
        delta_savings_bytes,
        delta_savings_pct,
        duplicate_representation_bytes: storage.duplicate_representation_bytes,
        metadata_overhead_bytes,
        actual_physical_bytes: storage.physical_bytes,
        net_physical_savings_bytes,
        net_physical_savings_pct,
    })
}

pub fn analytics_report(store_root: &Path) -> anyhow::Result<RepositoryAnalytics> {
    let cas = LocalCas::open(store_root)?;
    let manifests = load_manifests(store_root)?;
    let efficiency = storage_efficiency_report(store_root)?;

    // Count each logical object once per artifact, not once per repeated chunk.
    let mut reference_counts = HashMap::<String, usize>::new();
    for (_, manifest) in &manifests {
        let ids = manifest.chunks.iter().map(|c| c.object.as_str()).collect::<HashSet<_>>();
        for id in ids {
            *reference_counts.entry(id.to_string()).or_default() += 1;
        }
    }

    let mut artifacts = Vec::with_capacity(manifests.len());
    for (_, manifest) in &manifests {
        let mut exclusive_bytes = 0u64;
        let mut shared_bytes = 0u64;
        let mut attributed = 0f64;
        let mut attributed_ids = HashSet::<&str>::new();
        for chunk in &manifest.chunks {
            let refs = reference_counts.get(&chunk.object).copied().unwrap_or(1);
            if refs > 1 {
                shared_bytes = shared_bytes.saturating_add(chunk.size);
            } else {
                exclusive_bytes = exclusive_bytes.saturating_add(chunk.size);
            }
            if attributed_ids.insert(chunk.object.as_str()) {
                let oid = ObjectId::parse(&chunk.object)?;
                let physical = cas.estimated_primary_physical_size(&oid)?;
                attributed += physical as f64 / refs as f64;
            }
        }
        let shared_pct = pct(shared_bytes, manifest.logical_size);
        artifacts.push(ArtifactAnalytics {
            source_name: manifest.source_name.clone(),
            logical_bytes: manifest.logical_size,
            exclusive_bytes,
            shared_bytes,
            attributed_physical_bytes: attributed.round() as u64,
            shared_pct,
        });
    }

    let physical_ratio = if efficiency.logical_bytes == 0 {
        0.0
    } else {
        efficiency.actual_physical_bytes as f64 / efficiency.logical_bytes as f64 * 100.0
    };
    Ok(RepositoryAnalytics {
        artifacts,
        total_logical_bytes: efficiency.logical_bytes,
        unique_logical_object_bytes: efficiency.unique_logical_bytes,
        physical_bytes: efficiency.actual_physical_bytes,
        dedup_savings_bytes: efficiency.dedup_savings_bytes,
        dedup_savings_pct: efficiency.dedup_savings_pct,
        physical_ratio,
        efficiency,
    })
}

pub fn benchmark_snapshot(store_root: &Path) -> anyhow::Result<RepositoryBenchmarkSnapshot> {
    let storage = storage_report(store_root)?;
    let analytics = analytics_report(store_root)?;
    Ok(RepositoryBenchmarkSnapshot {
        format_version: 1,
        manifests: storage.manifests,
        objects: storage.reachable_objects,
        efficiency: analytics.efficiency,
        artifacts: analytics.artifacts,
    })
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 / whole as f64 * 100.0 }
}
