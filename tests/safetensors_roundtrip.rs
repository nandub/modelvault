use std::{collections::HashMap, fs, process::Command};

use modelvault::{
    artifact::{add_safetensors_artifact, materialize, materialize_selected_safetensors, resolve_selected_tensor_names},
    cas::LocalCas,
    pointer::ArtifactPointer,
};
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

#[test]
fn tensor_prefix_selectors_are_resolved_in_manifest_order() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("model.safetensors");
    let a = vec![1u8; 32];
    let b = vec![2u8; 32];
    let c = vec![3u8; 32];
    let mut tensors = HashMap::new();
    tensors.insert("encoder.a", TensorView::new(Dtype::U8, vec![32], &a).unwrap());
    tensors.insert("encoder.b", TensorView::new(Dtype::U8, vec![32], &b).unwrap());
    tensors.insert("head.weight", TensorView::new(Dtype::U8, vec![32], &c).unwrap());
    serialize_to_file(tensors, None, &source).unwrap();
    let cas = LocalCas::open(temp.path().join(".modelvault")).unwrap();
    let added = add_safetensors_artifact(&source, &cas, 32).unwrap();

    let selected = resolve_selected_tensor_names(&added.manifest, &["head.weight".into()], &["encoder.".into()]).unwrap();
    assert_eq!(selected, vec!["encoder.a", "encoder.b", "head.weight"]);
    assert!(resolve_selected_tensor_names(&added.manifest, &[], &["missing.".into()]).is_err());
    assert!(resolve_selected_tensor_names(&added.manifest, &[], &[String::new()]).is_err());
}

#[test]
fn extract_tensors_can_import_a_derived_artifact_and_record_lineage() {
    let repo = tempdir().unwrap();
    let source = repo.path().join("source.safetensors");
    let output = repo.path().join("exports").join("selected.safetensors");
    let a = vec![1u8; 8192];
    let b = vec![2u8; 5000];
    let mut tensors = HashMap::new();
    tensors.insert("layer.a", TensorView::new(Dtype::U8, vec![8192], &a).unwrap());
    tensors.insert("layer.b", TensorView::new(Dtype::U8, vec![5000], &b).unwrap());
    serialize_to_file(tensors, None, &source).unwrap();

    let binary = env!("CARGO_BIN_EXE_modelvault");
    assert!(Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(repo.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new(binary)
        .args(["track", "source.safetensors", "--pointer", "models/source.safetensors.mvptr"])
        .current_dir(repo.path())
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new(binary)
        .args([
            "extract-tensors",
            "models/source.safetensors.mvptr",
            "--prefix",
            "layer.",
            "--output",
            output.to_str().unwrap(),
            "--to",
            "models/derived/selected.safetensors",
        ])
        .current_dir(repo.path())
        .output()
        .unwrap()
        .status
        .success());

    let source_pointer = ArtifactPointer::load(repo.path().join("models/source.safetensors.mvptr")).unwrap();
    let derived_pointer = ArtifactPointer::load(repo.path().join("models/derived/selected.safetensors.mvptr")).unwrap();
    let (_, derived) = derived_pointer.resolve_manifest(repo.path()).unwrap();
    assert_eq!(derived.lineage.len(), 1);
    assert_eq!(derived.lineage[0].parent_artifact_id, source_pointer.artifact_id);
    assert_eq!(derived.lineage[0].operation, "extract-tensors");

    let bytes = fs::read(output).unwrap();
    let selected = SafeTensors::deserialize(&bytes).unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(selected.tensor("layer.a").unwrap().data(), a.as_slice());
    assert_eq!(selected.tensor("layer.b").unwrap().data(), b.as_slice());
}
