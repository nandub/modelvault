use std::{collections::HashMap, fs};

use modelvault::{artifact::{add_safetensors_artifact, materialize, materialize_selected_safetensors}, cas::LocalCas};
use safetensors::{tensor::{serialize_to_file, Dtype, TensorView}, SafeTensors};
use tempfile::tempdir;

#[test]
fn safetensors_artifact_round_trips_and_records_tensor_boundaries() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("model.safetensors");
    let output = temp.path().join("restored.safetensors");

    let a = vec![1u8; 8192];
    let b = vec![2u8; 5000];
    let mut tensors = HashMap::new();
    tensors.insert("layer.a", TensorView::new(Dtype::U8, vec![8192], &a).unwrap());
    tensors.insert("layer.b", TensorView::new(Dtype::U8, vec![5000], &b).unwrap());
    serialize_to_file(tensors, None, &source).unwrap();

    let cas = LocalCas::open(temp.path().join(".modelvault")).unwrap();
    let added = add_safetensors_artifact(&source, &cas, 1024).unwrap();
    assert_eq!(added.manifest.format, "safetensors");
    assert_eq!(added.manifest.tensors.len(), 2);
    assert!(added.manifest.chunks.iter().any(|c| c.tensor.as_deref() == Some("layer.a")));
    assert!(added.manifest.chunks.iter().any(|c| c.tensor.as_deref() == Some("layer.b")));

    materialize(&added.manifest, &cas, &output).unwrap();
    assert_eq!(fs::read(source).unwrap(), fs::read(output).unwrap());
}

#[test]
fn selected_tensors_materialize_as_a_valid_derived_safetensors_file() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("model.safetensors");
    let output = temp.path().join("selected.safetensors");
    let a = vec![1u8; 8192];
    let b = vec![2u8; 5000];
    let mut tensors = HashMap::new();
    tensors.insert("layer.a", TensorView::new(Dtype::U8, vec![8192], &a).unwrap());
    tensors.insert("layer.b", TensorView::new(Dtype::U8, vec![5000], &b).unwrap());
    serialize_to_file(tensors, None, &source).unwrap();

    let cas = LocalCas::open(temp.path().join(".modelvault")).unwrap();
    let added = add_safetensors_artifact(&source, &cas, 1024).unwrap();
    let result = materialize_selected_safetensors(&added.manifest, &cas, &["layer.b".to_string()], &output).unwrap();

    let bytes = fs::read(&output).unwrap();
    let selected = SafeTensors::deserialize(&bytes).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected.tensor("layer.b").unwrap().data(), b.as_slice());
    assert!(selected.tensor("layer.a").is_err());
    assert_eq!(result.source_artifact_id, added.manifest.artifact_id);
    assert_eq!(result.derived_artifact_id, blake3::hash(&bytes).to_hex().to_string());
}
