use std::collections::HashMap;

use anyhow::Context;

use crate::{
    cas::{LocalCas, ObjectId},
    manifest::ArtifactManifest,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeltaAnalysisReport {
    pub right_changed_chunks: usize,
    pub comparable_chunks: usize,
    pub incomparable_bytes: u64,
    pub full_compressed_bytes: u64,
    pub delta_compressed_bytes: u64,
    pub potential_savings_bytes: u64,
    pub potential_savings_pct: f64,
}

pub fn analyze_delta_potential(
    left: &ArtifactManifest,
    right: &ArtifactManifest,
    cas: &LocalCas,
    zstd_level: i32,
) -> anyhow::Result<DeltaAnalysisReport> {
    let left_by_range = left
        .chunks
        .iter()
        .map(|chunk| ((chunk.offset, chunk.size), chunk))
        .collect::<HashMap<_, _>>();

    let mut report = DeltaAnalysisReport::default();
    for right_chunk in &right.chunks {
        let left_chunk = match left_by_range.get(&(right_chunk.offset, right_chunk.size)) {
            Some(chunk) => *chunk,
            None => {
                report.incomparable_bytes =
                    report.incomparable_bytes.saturating_add(right_chunk.size);
                continue;
            }
        };
        if left_chunk.object == right_chunk.object {
            continue;
        }
        report.right_changed_chunks += 1;
        let left_id = ObjectId::parse(&left_chunk.object)?;
        let right_id = ObjectId::parse(&right_chunk.object)?;
        let left_bytes = cas
            .read(&left_id)
            .with_context(|| format!("failed to read left object {left_id}"))?;
        let right_bytes = cas
            .read(&right_id)
            .with_context(|| format!("failed to read right object {right_id}"))?;
        if left_bytes.len() != right_bytes.len() {
            report.incomparable_bytes = report.incomparable_bytes.saturating_add(right_chunk.size);
            continue;
        }
        report.comparable_chunks += 1;
        let delta = left_bytes
            .iter()
            .zip(&right_bytes)
            .map(|(a, b)| a ^ b)
            .collect::<Vec<_>>();

        #[cfg(feature = "compression")]
        {
            let full = zstd::stream::encode_all(std::io::Cursor::new(&right_bytes), zstd_level)?;
            let delta_encoded = zstd::stream::encode_all(std::io::Cursor::new(&delta), zstd_level)?;
            report.full_compressed_bytes = report
                .full_compressed_bytes
                .saturating_add(full.len() as u64);
            report.delta_compressed_bytes = report
                .delta_compressed_bytes
                .saturating_add(delta_encoded.len() as u64);
        }
        #[cfg(not(feature = "compression"))]
        {
            let _ = zstd_level;
            let _ = delta;
            anyhow::bail!("delta analysis requires the 'compression' feature");
        }
    }
    report.potential_savings_bytes = report
        .full_compressed_bytes
        .saturating_sub(report.delta_compressed_bytes);
    report.potential_savings_pct = if report.full_compressed_bytes == 0 {
        0.0
    } else {
        report.potential_savings_bytes as f64 / report.full_compressed_bytes as f64 * 100.0
    };
    Ok(report)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeltaOptimizeReport {
    pub candidates: usize,
    pub stored: usize,
    pub skipped: usize,
    pub full_physical_bytes: u64,
    pub delta_physical_bytes: u64,
    pub savings_bytes: u64,
    pub savings_pct: f64,
    pub max_depth_observed: u8,
}

pub fn optimize_delta_storage(
    left: &ArtifactManifest,
    right: &ArtifactManifest,
    cas: &LocalCas,
    zstd_level: i32,
    min_savings_pct: u8,
    max_depth: u8,
) -> anyhow::Result<DeltaOptimizeReport> {
    let left_by_range = left
        .chunks
        .iter()
        .map(|chunk| ((chunk.offset, chunk.size), chunk))
        .collect::<HashMap<_, _>>();
    let mut seen_targets = std::collections::HashSet::new();
    let mut report = DeltaOptimizeReport::default();

    for right_chunk in &right.chunks {
        let Some(left_chunk) = left_by_range
            .get(&(right_chunk.offset, right_chunk.size))
            .copied()
        else {
            continue;
        };
        if left_chunk.object == right_chunk.object
            || !seen_targets.insert(right_chunk.object.clone())
        {
            continue;
        }
        report.candidates += 1;
        let target = ObjectId::parse(&right_chunk.object)?;
        let base = ObjectId::parse(&left_chunk.object)?;
        let result =
            cas.optimize_object_as_delta(&target, &base, zstd_level, min_savings_pct, max_depth)?;
        report.max_depth_observed = report.max_depth_observed.max(result.depth);
        if result.stored {
            report.stored += 1;
            report.full_physical_bytes = report
                .full_physical_bytes
                .saturating_add(result.full_physical_bytes);
            report.delta_physical_bytes = report
                .delta_physical_bytes
                .saturating_add(result.delta_physical_bytes);
            report.savings_bytes = report.savings_bytes.saturating_add(result.savings_bytes);
        } else {
            report.skipped += 1;
        }
    }

    report.savings_pct = if report.full_physical_bytes == 0 {
        0.0
    } else {
        report.savings_bytes as f64 / report.full_physical_bytes as f64 * 100.0
    };
    Ok(report)
}
