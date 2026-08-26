use modelvault::{diff::diff_models, manifest::{ArtifactManifest,ChunkRef,TensorManifest}};

fn manifest(id:&str,obj:&str)->ArtifactManifest { ArtifactManifest { version:1, artifact_id:id.into(), format:"safetensors".into(), source_name:format!("{id}.safetensors"), logical_size:4, chunk_size:4, provenance:None, lineage:vec![], chunks:vec![ChunkRef{object:obj.into(),offset:0,size:4,tensor:Some("layer.weight".into())}], tensors:vec![TensorManifest{name:"layer.weight".into(),dtype:"U8".into(),shape:vec![4],data_offset:0,data_size:4}] } }

#[test]
fn model_diff_detects_changed_tensor() { let a=manifest("a","111"); let b=manifest("b","222"); let d=diff_models(&a,&b); assert_eq!(d.changed,1); assert_eq!(d.unchanged,0); }
#[test]
fn model_diff_detects_unchanged_tensor() { let a=manifest("a","111"); let b=manifest("b","111"); let d=diff_models(&a,&b); assert_eq!(d.unchanged,1); }
