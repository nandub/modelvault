use std::{fs::File, path::Path};

use memmap2::MmapOptions;
use safetensors::SafeTensors;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TensorInspection {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<usize>,
    pub byte_len: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SafetensorsInspection {
    pub format: &'static str,
    pub file_size: u64,
    pub tensor_count: usize,
    pub tensors: Vec<TensorInspection>,
}

pub fn inspect_safetensors(path: impl AsRef<Path>) -> anyhow::Result<SafetensorsInspection> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();

    // SAFETY: the mapping is read-only and remains valid for the lifetime of `file`/`mmap`.
    let mmap = unsafe { MmapOptions::new().map(&file)? };
    let container = SafeTensors::deserialize(&mmap)?;

    let mut tensors = Vec::with_capacity(container.len());
    for (name, tensor) in container.iter() {
        tensors.push(TensorInspection {
            name: name.to_owned(),
            dtype: format!("{:?}", tensor.dtype()),
            shape: tensor.shape().to_vec(),
            byte_len: tensor.data().len(),
        });
    }
    tensors.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(SafetensorsInspection {
        format: "safetensors",
        file_size,
        tensor_count: tensors.len(),
        tensors,
    })
}
