mod materialize;
mod safetensors;
mod store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactProgressPhase {
    Hashing,
    Storing,
    Materializing,
    Verifying,
}

pub type ArtifactProgressCallback<'a> = dyn FnMut(ArtifactProgressPhase, u64, u64) + 'a;

pub use materialize::{
    materialize, materialize_selected_safetensors, materialize_with_progress,
    resolve_selected_tensor_names, verify_artifact, SelectedTensorMaterialization,
};
pub use safetensors::{inspect_safetensors, SafetensorsInspection, TensorInspection};
pub use store::{
    add_raw_artifact, add_raw_artifact_with_progress, add_safetensors_artifact,
    add_safetensors_artifact_with_progress, hash_file, AddArtifactResult,
};
