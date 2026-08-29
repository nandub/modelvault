use crate::manifest::{ArtifactLineageEdge, ArtifactManifest, ArtifactProvenance, TensorManifest};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Serialize)]
pub struct TensorDiff {
    pub name: String,
    pub status: &'static str,
    pub left: Option<TensorManifest>,
    pub right: Option<TensorManifest>,
    pub shared_bytes: u64,
    pub right_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelDiff {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub tensors: Vec<TensorDiff>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportArtifact {
    pub artifact_id: String,
    pub source_name: String,
    pub logical_size: u64,
    pub provenance: Option<ArtifactProvenance>,
    pub lineage: Vec<ArtifactLineageEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelDiffReport {
    pub version: u32,
    pub left: ReportArtifact,
    pub right: ReportArtifact,
    pub added_bytes: u64,
    pub removed_bytes: u64,
    pub changed_right_bytes: u64,
    pub reused_right_bytes: u64,
    pub diff: ModelDiff,
}

pub fn diff_report(left: &ArtifactManifest, right: &ArtifactManifest) -> ModelDiffReport {
    let diff = diff_models(left, right);
    let added_bytes = diff
        .tensors
        .iter()
        .filter(|t| t.status == "added")
        .map(|t| t.right_bytes)
        .sum();
    let removed_bytes = diff
        .tensors
        .iter()
        .filter_map(|t| {
            (t.status == "removed").then_some(t.left.as_ref().map_or(0, |v| v.data_size))
        })
        .sum();
    let changed_right_bytes = diff
        .tensors
        .iter()
        .filter(|t| t.status == "changed")
        .map(|t| t.right_bytes)
        .sum();
    let reused_right_bytes = diff.tensors.iter().map(|t| t.shared_bytes).sum();
    let artifact = |m: &ArtifactManifest| ReportArtifact {
        artifact_id: m.artifact_id.clone(),
        source_name: m.source_name.clone(),
        logical_size: m.logical_size,
        provenance: m.provenance.clone(),
        lineage: m.lineage.clone(),
    };
    ModelDiffReport {
        version: 1,
        left: artifact(left),
        right: artifact(right),
        added_bytes,
        removed_bytes,
        changed_right_bytes,
        reused_right_bytes,
        diff,
    }
}

fn tensor_objects(m: &ArtifactManifest) -> HashMap<&str, Vec<(&str, u64)>> {
    let mut result: HashMap<&str, Vec<(&str, u64)>> = HashMap::new();
    for c in &m.chunks {
        if let Some(name) = c.tensor.as_deref() {
            result
                .entry(name)
                .or_default()
                .push((c.object.as_str(), c.size));
        }
    }
    result
}

pub fn diff_models(left: &ArtifactManifest, right: &ArtifactManifest) -> ModelDiff {
    let lm: BTreeMap<&str, &TensorManifest> =
        left.tensors.iter().map(|t| (t.name.as_str(), t)).collect();
    let rm: BTreeMap<&str, &TensorManifest> =
        right.tensors.iter().map(|t| (t.name.as_str(), t)).collect();
    let lo = tensor_objects(left);
    let ro = tensor_objects(right);
    let names: BTreeMap<&str, ()> = lm.keys().chain(rm.keys()).map(|n| (*n, ())).collect();
    let mut out = ModelDiff {
        added: 0,
        removed: 0,
        changed: 0,
        unchanged: 0,
        tensors: Vec::new(),
    };
    for name in names.keys() {
        let l = lm.get(name).copied();
        let r = rm.get(name).copied();
        let (status, shared, right_bytes) = match (l, r) {
            (None, Some(rt)) => {
                out.added += 1;
                ("added", 0, rt.data_size)
            }
            (Some(_), None) => {
                out.removed += 1;
                ("removed", 0, 0)
            }
            (Some(lt), Some(rt)) => {
                let lseq = lo.get(name).cloned().unwrap_or_default();
                let rseq = ro.get(name).cloned().unwrap_or_default();
                if lt.dtype == rt.dtype && lt.shape == rt.shape && lseq == rseq {
                    out.unchanged += 1;
                    ("unchanged", rt.data_size, rt.data_size)
                } else {
                    out.changed += 1;
                    let lset: HashSet<&str> = lseq.iter().map(|(id, _)| *id).collect();
                    let shared = rseq
                        .iter()
                        .filter(|(id, _)| lset.contains(id))
                        .map(|(_, s)| *s)
                        .sum();
                    ("changed", shared, rt.data_size)
                }
            }
            (None, None) => unreachable!(),
        };
        out.tensors.push(TensorDiff {
            name: (*name).to_owned(),
            status,
            left: l.cloned(),
            right: r.cloned(),
            shared_bytes: shared,
            right_bytes,
        });
    }
    out
}
