use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    time::Instant,
};

use anyhow::Context;
use serde::Serialize;

use crate::{
    cas::{LocalCas, ObjectId},
    chunk::fixed::chunk_bytes,
    manifest::ArtifactManifest,
    repository::RepositoryBenchmarkSnapshot,
};

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkDelta {
    pub logical_bytes: i128,
    pub physical_bytes: i128,
    pub dedup_savings_pct_points: f64,
    pub compression_savings_pct_points: f64,
    pub delta_savings_pct_points: f64,
    pub net_savings_pct_points: f64,
}

pub fn compare_snapshots(
    left: &RepositoryBenchmarkSnapshot,
    right: &RepositoryBenchmarkSnapshot,
) -> BenchmarkDelta {
    BenchmarkDelta {
        logical_bytes: right.efficiency.logical_bytes as i128
            - left.efficiency.logical_bytes as i128,
        physical_bytes: right.efficiency.actual_physical_bytes as i128
            - left.efficiency.actual_physical_bytes as i128,
        dedup_savings_pct_points: right.efficiency.dedup_savings_pct
            - left.efficiency.dedup_savings_pct,
        compression_savings_pct_points: right.efficiency.compression_savings_pct
            - left.efficiency.compression_savings_pct,
        delta_savings_pct_points: right.efficiency.delta_savings_pct
            - left.efficiency.delta_savings_pct,
        net_savings_pct_points: right.efficiency.net_physical_savings_pct
            - left.efficiency.net_physical_savings_pct,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkStats {
    pub artifact: String,
    pub chunks: usize,
    pub logical_bytes: u64,
    pub min_bytes: u64,
    pub max_bytes: u64,
    pub average_bytes: f64,
    pub median_bytes: f64,
    pub exact_duplicate_chunks: usize,
    pub exact_duplicate_bytes: u64,
    pub compressed_smaller_chunks: usize,
    pub raw_preferred_chunks: usize,
    pub full_encoded_bytes: u64,
    pub compression_savings_pct: f64,
}

pub fn chunk_stats(
    manifest: &ArtifactManifest,
    cas: &LocalCas,
    level: i32,
) -> anyhow::Result<ChunkStats> {
    let mut sizes = manifest.chunks.iter().map(|c| c.size).collect::<Vec<_>>();
    sizes.sort_unstable();
    let logical_bytes = sizes.iter().sum::<u64>();
    let min_bytes = sizes.first().copied().unwrap_or(0);
    let max_bytes = sizes.last().copied().unwrap_or(0);
    let average_bytes = if sizes.is_empty() {
        0.0
    } else {
        logical_bytes as f64 / sizes.len() as f64
    };
    let median_bytes = match sizes.len() {
        0 => 0.0,
        n if n % 2 == 1 => sizes[n / 2] as f64,
        n => (sizes[n / 2 - 1] as f64 + sizes[n / 2] as f64) / 2.0,
    };
    let mut counts: HashMap<&str, (usize, u64)> = HashMap::new();
    for c in &manifest.chunks {
        let e = counts.entry(&c.object).or_insert((0, c.size));
        e.0 += 1;
    }
    let exact_duplicate_chunks = counts.values().map(|(n, _)| n.saturating_sub(1)).sum();
    let exact_duplicate_bytes = counts
        .values()
        .map(|(n, s)| n.saturating_sub(1) as u64 * *s)
        .sum();
    let mut compressed_smaller_chunks = 0usize;
    let mut raw_preferred_chunks = 0usize;
    let mut full_encoded_bytes = 0u64;
    for object in counts.keys() {
        let id = ObjectId::parse(object)?;
        let bytes = cas.read(&id)?;
        #[cfg(feature = "compression")]
        let compressed = zstd::bulk::compress(&bytes, level)?;
        #[cfg(not(feature = "compression"))]
        let compressed = bytes.clone();
        if compressed.len() < bytes.len() {
            compressed_smaller_chunks += 1;
            full_encoded_bytes += compressed.len() as u64;
        } else {
            raw_preferred_chunks += 1;
            full_encoded_bytes += bytes.len() as u64;
        }
    }
    let unique_logical = counts.values().map(|(_, s)| *s).sum::<u64>();
    let compression_savings_pct = if unique_logical == 0 {
        0.0
    } else {
        (unique_logical.saturating_sub(full_encoded_bytes)) as f64 / unique_logical as f64 * 100.0
    };
    Ok(ChunkStats {
        artifact: manifest.source_name.clone(),
        chunks: sizes.len(),
        logical_bytes,
        min_bytes,
        max_bytes,
        average_bytes,
        median_bytes,
        exact_duplicate_chunks,
        exact_duplicate_bytes,
        compressed_smaller_chunks,
        raw_preferred_chunks,
        full_encoded_bytes,
        compression_savings_pct,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PairChunkStats {
    pub left: ChunkStats,
    pub right: ChunkStats,
    pub shared_objects: usize,
    pub shared_bytes: u64,
}

pub fn pair_chunk_stats(
    left: &ArtifactManifest,
    right: &ArtifactManifest,
    cas: &LocalCas,
    level: i32,
) -> anyhow::Result<PairChunkStats> {
    let lset = left
        .chunks
        .iter()
        .map(|c| (c.object.as_str(), c.size))
        .collect::<HashMap<_, _>>();
    let rset = right
        .chunks
        .iter()
        .map(|c| (c.object.as_str(), c.size))
        .collect::<HashMap<_, _>>();
    let shared = lset
        .keys()
        .filter(|k| rset.contains_key(*k))
        .collect::<Vec<_>>();
    let shared_bytes = shared.iter().filter_map(|k| lset.get(*k)).copied().sum();
    Ok(PairChunkStats {
        left: chunk_stats(left, cas, level)?,
        right: chunk_stats(right, cas, level)?,
        shared_objects: shared.len(),
        shared_bytes,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicySimulationRow {
    pub chunk_size: usize,
    pub delta_threshold_pct: u8,
    pub chunks: usize,
    pub shared_chunks: usize,
    pub estimated_physical_bytes: u64,
    pub net_savings_pct: f64,
    pub elapsed_ms: u128,
}

pub fn simulate_policy(
    left: &Path,
    right: &Path,
    chunk_sizes: &[usize],
    thresholds: &[u8],
    level: i32,
) -> anyhow::Result<Vec<PolicySimulationRow>> {
    let lb = fs::read(left).with_context(|| format!("failed to read {}", left.display()))?;
    let rb = fs::read(right).with_context(|| format!("failed to read {}", right.display()))?;
    let logical = lb.len() as u64 + rb.len() as u64;
    let mut rows = Vec::new();
    for &chunk_size in chunk_sizes {
        anyhow::ensure!(chunk_size > 0, "chunk size must be > 0");
        let started = Instant::now();
        let lr = chunk_bytes(lb.len() as u64, chunk_size);
        let rr = chunk_bytes(rb.len() as u64, chunk_size);
        let mut left_full: HashMap<[u8; 32], (Vec<u8>, u64)> = HashMap::new();
        let mut base_physical = 0u64;
        for r in &lr {
            let b = &lb[r.offset as usize..r.offset as usize + r.len];
            let h = *blake3::hash(b).as_bytes();
            if left_full.contains_key(&h) {
                continue;
            }
            #[cfg(feature = "compression")]
            let c = zstd::bulk::compress(b, level)?;
            #[cfg(not(feature = "compression"))]
            let c = b.to_vec();
            let stored = c.len().min(b.len()) as u64;
            base_physical += stored;
            left_full.insert(h, (b.to_vec(), stored));
        }
        for &threshold in thresholds {
            let mut physical = base_physical;
            let mut shared = 0usize;
            let mut seen: HashSet<[u8; 32]> = left_full.keys().copied().collect();
            for (idx, r) in rr.iter().enumerate() {
                let b = &rb[r.offset as usize..r.offset as usize + r.len];
                let h = *blake3::hash(b).as_bytes();
                if seen.contains(&h) {
                    shared += 1;
                    continue;
                }
                seen.insert(h);
                #[cfg(feature = "compression")]
                let full = zstd::bulk::compress(b, level)?;
                #[cfg(not(feature = "compression"))]
                let full = b.to_vec();
                let full_len = full.len().min(b.len()) as u64;
                let mut chosen = full_len;
                if let Some(lr0) = lr.get(idx) {
                    if lr0.len == r.len {
                        let base = &lb[lr0.offset as usize..lr0.offset as usize + lr0.len];
                        let xor = base.iter().zip(b).map(|(a, b)| a ^ b).collect::<Vec<_>>();
                        #[cfg(feature = "compression")]
                        let d = zstd::bulk::compress(&xor, level)?;
                        #[cfg(not(feature = "compression"))]
                        let d = xor;
                        let delta_len = d.len() as u64 + 64;
                        let savings = if full_len == 0 {
                            0.0
                        } else {
                            (full_len.saturating_sub(delta_len)) as f64 / full_len as f64 * 100.0
                        };
                        if savings >= threshold as f64 {
                            chosen = chosen.min(delta_len);
                        }
                    }
                }
                physical += chosen;
            }
            let net = if logical == 0 {
                0.0
            } else {
                (logical.saturating_sub(physical)) as f64 / logical as f64 * 100.0
            };
            rows.push(PolicySimulationRow {
                chunk_size,
                delta_threshold_pct: threshold,
                chunks: lr.len() + rr.len(),
                shared_chunks: shared,
                estimated_physical_bytes: physical,
                net_savings_pct: net,
                elapsed_ms: started.elapsed().as_millis(),
            });
        }
    }
    Ok(rows)
}
