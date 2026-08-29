use anyhow::{bail, Context};
use memmap2::MmapOptions;
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::File,
    path::Path,
    time::{Duration, Instant},
};

use crate::chunk::{chunk_bytes, chunk_cdc, ChunkRange};

#[derive(Debug, Clone, Copy)]
pub enum Strategy {
    Fixed,
    TensorFixed,
    FastCdc,
    TensorFastCdc,
}

impl Strategy {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::TensorFixed => "tensor-fixed",
            Self::FastCdc => "fastcdc",
            Self::TensorFastCdc => "tensor-fastcdc",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchResult {
    pub strategy: &'static str,
    pub left_chunks: usize,
    pub right_chunks: usize,
    pub shared_bytes: u64,
    pub right_size: u64,
    pub reuse_pct: f64,
    pub elapsed: Duration,
}

#[derive(Debug, Deserialize)]
struct RawTensorHeader {
    data_offsets: [u64; 2],
}

fn tensor_regions(bytes: &[u8]) -> anyhow::Result<(u64, Vec<(u64, u64)>)> {
    if bytes.len() < 8 {
        bail!("Safetensors input is too small");
    }
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&bytes[..8]);
    let header_len = u64::from_le_bytes(prefix);
    let data_start = 8 + header_len;
    if data_start as usize > bytes.len() {
        bail!("Safetensors header extends beyond input");
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes[8..data_start as usize])?;
    let map = value
        .as_object()
        .context("Safetensors header must be an object")?;
    let mut regions = Vec::new();
    for (name, v) in map {
        if name == "__metadata__" {
            continue;
        }
        let raw: RawTensorHeader = serde_json::from_value(v.clone())?;
        regions.push((
            data_start + raw.data_offsets[0],
            raw.data_offsets[1] - raw.data_offsets[0],
        ));
    }
    regions.sort_by_key(|r| r.0);
    Ok((data_start, regions))
}

fn ranges(
    bytes: &[u8],
    strategy: Strategy,
    avg: usize,
    safetensors: bool,
) -> anyhow::Result<Vec<ChunkRange>> {
    match strategy {
        Strategy::Fixed => Ok(chunk_bytes(bytes.len() as u64, avg)),
        Strategy::FastCdc => Ok(chunk_cdc(bytes, avg)),
        Strategy::TensorFixed | Strategy::TensorFastCdc if safetensors => {
            let (data_start, tensors) = tensor_regions(bytes)?;
            let mut out = Vec::new();
            let header = &bytes[..data_start as usize];
            if matches!(strategy, Strategy::TensorFastCdc) {
                out.extend(chunk_cdc(header, avg));
            } else {
                out.extend(chunk_bytes(data_start, avg));
            }
            for (start, len) in tensors {
                let slice = &bytes[start as usize..(start + len) as usize];
                if matches!(strategy, Strategy::TensorFastCdc) {
                    out.extend(chunk_cdc(slice, avg).into_iter().map(|r| ChunkRange {
                        offset: start + r.offset,
                        len: r.len,
                    }));
                } else {
                    out.extend(chunk_bytes(len, avg).into_iter().map(|r| ChunkRange {
                        offset: start + r.offset,
                        len: r.len,
                    }));
                }
            }
            Ok(out)
        }
        Strategy::TensorFixed => Ok(chunk_bytes(bytes.len() as u64, avg)),
        Strategy::TensorFastCdc => Ok(chunk_cdc(bytes, avg)),
    }
}

fn hash_ranges(bytes: &[u8], ranges: &[ChunkRange]) -> Vec<(String, u64)> {
    ranges
        .iter()
        .map(|r| {
            let s = r.offset as usize;
            let e = s + r.len;
            (
                blake3::hash(&bytes[s..e]).to_hex().to_string(),
                r.len as u64,
            )
        })
        .collect()
}

pub fn benchmark_pair(
    left: &Path,
    right: &Path,
    avg: usize,
    safetensors: bool,
) -> anyhow::Result<Vec<BenchResult>> {
    let lf = File::open(left)?;
    let rf = File::open(right)?;
    let l = unsafe { MmapOptions::new().map(&lf)? };
    let r = unsafe { MmapOptions::new().map(&rf)? };
    let strategies = [
        Strategy::Fixed,
        Strategy::TensorFixed,
        Strategy::FastCdc,
        Strategy::TensorFastCdc,
    ];
    let mut results = Vec::new();
    for strategy in strategies {
        let started = Instant::now();
        let lr = ranges(&l, strategy, avg, safetensors)?;
        let rr = ranges(&r, strategy, avg, safetensors)?;
        let lh = hash_ranges(&l, &lr);
        let rh = hash_ranges(&r, &rr);
        let set: HashSet<&str> = lh.iter().map(|(h, _)| h.as_str()).collect();
        let shared: u64 = rh
            .iter()
            .filter(|(h, _)| set.contains(h.as_str()))
            .map(|(_, s)| *s)
            .sum();
        let size = r.len() as u64;
        results.push(BenchResult {
            strategy: strategy.name(),
            left_chunks: lr.len(),
            right_chunks: rr.len(),
            shared_bytes: shared,
            right_size: size,
            reuse_pct: if size == 0 {
                0.0
            } else {
                shared as f64 / size as f64 * 100.0
            },
            elapsed: started.elapsed(),
        });
    }
    Ok(results)
}
