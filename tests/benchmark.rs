use std::collections::HashMap;

use modelvault::benchmark::benchmark_pair;
use safetensors::tensor::{serialize_to_file, Dtype, TensorView};
use tempfile::tempdir;

#[test]
fn safetensors_benchmark_reports_all_chunking_strategies() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let left = temp.path().join("base.safetensors");
    let right = temp.path().join("tuned.safetensors");

    let left_weights = vec![1_u8; 8_192];
    let right_weights = vec![1_u8; 8_192];
    let left_bias = vec![2_u8; 4_096];
    let right_bias = vec![3_u8; 4_096];
    let mut left_tensors = HashMap::new();
    left_tensors.insert(
        "layer.weight",
        TensorView::new(Dtype::U8, vec![8_192], &left_weights)?,
    );
    left_tensors.insert(
        "layer.bias",
        TensorView::new(Dtype::U8, vec![4_096], &left_bias)?,
    );
    let mut right_tensors = HashMap::new();
    right_tensors.insert(
        "layer.weight",
        TensorView::new(Dtype::U8, vec![8_192], &right_weights)?,
    );
    right_tensors.insert(
        "layer.bias",
        TensorView::new(Dtype::U8, vec![4_096], &right_bias)?,
    );
    serialize_to_file(left_tensors, None, &left)?;
    serialize_to_file(right_tensors, None, &right)?;

    let rows = benchmark_pair(&left, &right, 1_024, true)?;

    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter().map(|row| row.strategy).collect::<Vec<_>>(),
        ["fixed", "tensor-fixed", "fastcdc", "tensor-fastcdc"]
    );
    assert!(rows.iter().all(|row| row.right_size > 0));
    assert!(rows.iter().all(|row| row.reuse_pct.is_finite()));
    assert!(rows.iter().all(|row| (0.0..=100.0).contains(&row.reuse_pct)));
    Ok(())
}
