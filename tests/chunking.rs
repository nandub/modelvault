use modelvault::chunk::{chunk_bytes, chunk_tensor_range};

#[test]
fn fixed_chunking_covers_all_bytes() {
    let chunks = chunk_bytes(10, 4);
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].offset, 0);
    assert_eq!(chunks[0].len, 4);
    assert_eq!(chunks[1].offset, 4);
    assert_eq!(chunks[1].len, 4);
    assert_eq!(chunks[2].offset, 8);
    assert_eq!(chunks[2].len, 2);
}

#[test]
fn tensor_chunks_never_leave_tensor_range() {
    let chunks = chunk_tensor_range("layer.0.weight", 100, 10, 4);
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].absolute.offset, 100);
    assert_eq!(chunks[1].absolute.offset, 104);
    assert_eq!(chunks[2].absolute.offset, 108);
    assert_eq!(chunks[2].absolute.len, 2);
}

#[test]
fn cdc_chunking_covers_all_bytes() {
    let data = vec![42u8; 256 * 1024];
    let chunks = modelvault::chunk::chunk_cdc(&data, 16 * 1024);
    assert!(!chunks.is_empty());
    assert_eq!(chunks.first().unwrap().offset, 0);
    let end = chunks.last().unwrap().offset + chunks.last().unwrap().len as u64;
    assert_eq!(end, data.len() as u64);
    for pair in chunks.windows(2) {
        assert_eq!(pair[0].offset + pair[0].len as u64, pair[1].offset);
    }
}
