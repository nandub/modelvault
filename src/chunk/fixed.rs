#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRange {
    pub offset: u64,
    pub len: usize,
}

pub fn chunk_bytes(total_len: u64, chunk_size: usize) -> Vec<ChunkRange> {
    assert!(chunk_size > 0, "chunk size must be greater than zero");

    let mut chunks = Vec::new();
    let mut offset = 0u64;
    while offset < total_len {
        let remaining = total_len - offset;
        let len = remaining.min(chunk_size as u64) as usize;
        chunks.push(ChunkRange { offset, len });
        offset += len as u64;
    }
    chunks
}
