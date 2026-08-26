use super::ChunkRange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorChunk {
    pub tensor_name: String,
    pub relative_offset: u64,
    pub absolute: ChunkRange,
}

pub fn chunk_tensor_range(
    tensor_name: impl Into<String>,
    absolute_start: u64,
    tensor_len: u64,
    chunk_size: usize,
) -> Vec<TensorChunk> {
    let tensor_name = tensor_name.into();
    super::chunk_bytes(tensor_len, chunk_size)
        .into_iter()
        .map(|range| TensorChunk {
            tensor_name: tensor_name.clone(),
            relative_offset: range.offset,
            absolute: ChunkRange {
                offset: absolute_start + range.offset,
                len: range.len,
            },
        })
        .collect()
}
