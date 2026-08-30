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

#[test]
fn cdc_chunking_has_stable_boundaries_for_a_deterministic_fixture() {
    // A deterministic pseudo-random fixture avoids the misleadingly regular
    // boundaries that repetitive data can produce, while keeping this test
    // self-contained and reproducible across platforms.
    let mut state = 0x4d56_4344_u64;
    let data = (0..512 * 1024)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (state >> 56) as u8
        })
        .collect::<Vec<_>>();

    let boundaries = modelvault::chunk::chunk_cdc(&data, 16 * 1024)
        .into_iter()
        .map(|chunk| (chunk.offset, chunk.len))
        .collect::<Vec<_>>();
    assert_eq!(
        boundaries,
        [
            (0, 23_787),
            (23_787, 14_730),
            (38_517, 10_877),
            (49_394, 32_811),
            (82_205, 12_609),
            (94_814, 17_298),
            (112_112, 9_310),
            (121_422, 20_720),
            (142_142, 25_403),
            (167_545, 18_616),
            (186_161, 23_741),
            (209_902, 19_928),
            (229_830, 23_804),
            (253_634, 15_484),
            (269_118, 24_900),
            (294_018, 9_339),
            (303_357, 10_303),
            (313_660, 7_764),
            (321_424, 11_990),
            (333_414, 8_374),
            (341_788, 18_821),
            (360_609, 10_292),
            (370_901, 11_836),
            (382_737, 17_495),
            (400_232, 27_015),
            (427_247, 12_689),
            (439_936, 16_589),
            (456_525, 50_762),
            (507_287, 17_001),
        ]
    );
}
