use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

use anyhow::{ensure, Context};

use crate::{
    cas::{LocalCas, ObjectId},
    manifest::ArtifactManifest,
    object_store::{FilesystemObjectStore, ObjectStore},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOptions {
    pub jobs: usize,
    /// Require destination object bodies to be read and BLAKE3-verified before reuse.
    /// This is slower for remote stores but does not trust metadata-only verification.
    pub deep_verify: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            jobs: 4,
            deep_verify: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncResult {
    /// Number of unique CAS objects required by the artifact.
    pub objects_total: usize,
    pub objects_copied: usize,
    pub objects_reused: usize,
    pub bytes_copied: u64,
    pub bytes_reused: u64,
    pub manifest_path: PathBuf,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone)]
struct TransferTask {
    id: ObjectId,
    size: u64,
}

#[derive(Debug, Clone)]
enum TransferOutcome {
    Copied { bytes: u64 },
    Reused { id: ObjectId },
}

/// Backward-compatible filesystem push API.
pub fn push_manifest(
    manifest: &ArtifactManifest,
    local: &LocalCas,
    remote_root: &Path,
) -> anyhow::Result<SyncResult> {
    push_manifest_with_options(manifest, local, remote_root, &SyncOptions::default())
}

pub fn push_manifest_with_options(
    manifest: &ArtifactManifest,
    local: &LocalCas,
    remote_root: &Path,
    options: &SyncOptions,
) -> anyhow::Result<SyncResult> {
    let source = FilesystemObjectStore::open(local.root())?;
    let destination = FilesystemObjectStore::open(remote_root)?;
    sync_manifest_between(manifest, &source, &destination, options)
}

/// Backward-compatible filesystem pull API.
pub fn pull_manifest(
    manifest: &ArtifactManifest,
    local: &LocalCas,
    remote_root: &Path,
) -> anyhow::Result<SyncResult> {
    pull_manifest_with_options(manifest, local, remote_root, &SyncOptions::default())
}

pub fn pull_manifest_with_options(
    manifest: &ArtifactManifest,
    local: &LocalCas,
    remote_root: &Path,
    options: &SyncOptions,
) -> anyhow::Result<SyncResult> {
    let source = FilesystemObjectStore::open(remote_root)?;
    let destination = FilesystemObjectStore::open(local.root())?;
    sync_manifest_between(manifest, &source, &destination, options)
}

/// Synchronize an artifact between any two object-store implementations.
///
/// Restartability is object-granular: verified destination objects are skipped.
/// Because CAS objects are immutable chunks, an interrupted synchronization can
/// simply be run again and only missing/corrupt objects are transferred.
pub fn sync_manifest_between(
    manifest: &ArtifactManifest,
    source: &dyn ObjectStore,
    destination: &dyn ObjectStore,
    options: &SyncOptions,
) -> anyhow::Result<SyncResult> {
    ensure!(options.jobs > 0, "sync jobs must be greater than zero");
    let started = Instant::now();

    let tasks = unique_tasks(manifest)?;
    let task_count = tasks.len();
    let tasks = Arc::new(Mutex::new(tasks.into_iter()));
    let outcomes = Arc::new(Mutex::new(Vec::<TransferOutcome>::with_capacity(
        task_count,
    )));
    let failure = Arc::new(Mutex::new(None::<anyhow::Error>));
    let workers = options.jobs.min(task_count.max(1));

    thread::scope(|scope| {
        for _ in 0..workers {
            let tasks = Arc::clone(&tasks);
            let outcomes = Arc::clone(&outcomes);
            let failure = Arc::clone(&failure);
            scope.spawn(move || loop {
                if failure.lock().expect("failure mutex poisoned").is_some() {
                    break;
                }
                let task = tasks.lock().expect("task mutex poisoned").next();
                let Some(task) = task else { break };
                match transfer_one(&task, source, destination, options.deep_verify) {
                    Ok(outcome) => outcomes
                        .lock()
                        .expect("outcome mutex poisoned")
                        .push(outcome),
                    Err(err) => {
                        *failure.lock().expect("failure mutex poisoned") = Some(err);
                        break;
                    }
                }
            });
        }
    });

    if let Some(err) = failure.lock().expect("failure mutex poisoned").take() {
        return Err(err);
    }

    let outcomes = outcomes.lock().expect("outcome mutex poisoned");
    let mut objects_copied = 0usize;
    let mut objects_reused = 0usize;
    let mut bytes_copied = 0u64;
    for outcome in outcomes.iter().cloned() {
        match outcome {
            TransferOutcome::Copied { bytes } => {
                objects_copied += 1;
                bytes_copied = bytes_copied.saturating_add(bytes);
            }
            TransferOutcome::Reused { .. } => {
                objects_reused += 1;
            }
        }
    }

    let reused_ids = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            TransferOutcome::Reused { id } => Some(id.as_str()),
            TransferOutcome::Copied { .. } => None,
        })
        .collect::<std::collections::HashSet<_>>();
    let bytes_reused = manifest
        .chunks
        .iter()
        .filter(|chunk| reused_ids.contains(chunk.object.as_str()))
        .map(|chunk| chunk.size)
        .sum();

    ensure!(
        objects_copied + objects_reused == task_count,
        "synchronization ended before all objects were processed"
    );

    let manifest_path = destination.save_manifest(manifest)?;
    Ok(SyncResult {
        objects_total: task_count,
        objects_copied,
        objects_reused,
        bytes_copied,
        bytes_reused,
        manifest_path,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn unique_tasks(manifest: &ArtifactManifest) -> anyhow::Result<Vec<TransferTask>> {
    let mut unique = BTreeMap::<String, u64>::new();
    for chunk in &manifest.chunks {
        let id = ObjectId::parse(&chunk.object)?;
        match unique.get(id.as_str()) {
            Some(existing) => ensure!(
                *existing == chunk.size,
                "object {} appears with inconsistent sizes {} and {}",
                id,
                existing,
                chunk.size
            ),
            None => {
                unique.insert(id.to_string(), chunk.size);
            }
        }
    }
    unique
        .into_iter()
        .map(|(id, size)| {
            Ok(TransferTask {
                id: ObjectId::parse(&id)?,
                size,
            })
        })
        .collect()
}

fn transfer_one(
    task: &TransferTask,
    source: &dyn ObjectStore,
    destination: &dyn ObjectStore,
    deep_verify: bool,
) -> anyhow::Result<TransferOutcome> {
    let destination_valid = |store: &dyn ObjectStore, id: &ObjectId| {
        if deep_verify {
            store.verify_deep(id)
        } else {
            store.verify(id)
        }
    };
    if destination.contains(&task.id)? && destination_valid(destination, &task.id)? {
        return Ok(TransferOutcome::Reused {
            id: task.id.clone(),
        });
    }

    let bytes = source.read(&task.id).with_context(|| {
        format!(
            "source '{}' is missing CAS object {}",
            source.display_name(),
            task.id
        )
    })?;
    ensure!(
        bytes.len() as u64 == task.size,
        "source CAS object {} has {} bytes; manifest expects {}",
        task.id,
        bytes.len(),
        task.size
    );
    ensure!(
        ObjectId::from_bytes(&bytes) == task.id,
        "source CAS object {} failed BLAKE3 verification",
        task.id
    );

    if destination.contains(&task.id)? && !destination_valid(destination, &task.id)? {
        destination.remove(&task.id)?;
    }

    let put = destination.put_bytes(&bytes)?;
    ensure!(
        put.id == task.id,
        "destination returned unexpected object id {} for {}",
        put.id,
        task.id
    );
    ensure!(
        put.size == task.size,
        "destination returned unexpected size {} for {}",
        put.size,
        task.id
    );
    if put.was_new {
        if deep_verify {
            ensure!(
                destination.verify_deep(&task.id)?,
                "destination object {} failed deep verification after put",
                task.id
            );
        }
        Ok(TransferOutcome::Copied { bytes: task.size })
    } else {
        // Another worker/process may have won the race.
        ensure!(
            destination_valid(destination, &task.id)?,
            "destination object {} failed verification after put",
            task.id
        );
        Ok(TransferOutcome::Reused {
            id: task.id.clone(),
        })
    }
}
