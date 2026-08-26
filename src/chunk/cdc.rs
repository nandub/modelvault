use fastcdc::v2020::FastCDC;

use super::fixed::ChunkRange;

pub fn chunk_cdc(data: &[u8], avg_size: usize) -> Vec<ChunkRange> {
    if data.is_empty() { return Vec::new(); }
    let avg = avg_size.max(64);
    let min = (avg / 4).max(64);
    let max = avg.saturating_mul(4).max(avg);
    FastCDC::new(data, min, avg, max)
        .map(|c| ChunkRange { offset: c.offset as u64, len: c.length })
        .collect()
}

pub fn chunk_cdc_range(data: &[u8], absolute_start: u64, avg_size: usize) -> Vec<ChunkRange> {
    chunk_cdc(data, avg_size)
        .into_iter()
        .map(|r| ChunkRange { offset: absolute_start + r.offset, len: r.len })
        .collect()
}
