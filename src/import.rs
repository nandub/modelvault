use std::{
    env,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};

pub fn repository_target_path(root: &Path, requested: &Path) -> anyhow::Result<PathBuf> {
    // Preserve the caller-facing repository path for returned/logged paths. On Windows,
    // fs::canonicalize() may introduce a verbatim `\\?\` prefix; that form is useful
    // for containment checks but should not leak into normal ModelVault paths.
    let display_root = root.to_path_buf();
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| display_root.clone());

    if !requested.is_absolute()
        && requested
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("--to must be a repository-relative path without '..'");
    }

    let target = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        display_root.join(requested)
    };
    let parent = target.parent().unwrap_or(&display_root);

    // Validate the nearest existing ancestor before creating directories. This avoids
    // leaving filesystem side effects for a rejected target and catches symlink escapes.
    let mut existing = parent;
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| anyhow::anyhow!("--to has no existing parent"))?;
    }
    let canonical_existing = fs::canonicalize(existing)?;
    if !canonical_existing.starts_with(&canonical_root) {
        bail!("--to must resolve inside the Git repository");
    }

    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(&canonical_root) {
        bail!("--to must resolve inside the Git repository");
    }
    Ok(target)
}

pub fn pointer_path_for_target(target: &Path) -> PathBuf {
    crate::pointer::ArtifactPointer::default_pointer_path(target)
}

pub fn default_hf_target(repo_id: &str, filename: &str) -> PathBuf {
    let repo_name = repo_id
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("huggingface-model");
    PathBuf::from("models").join(repo_name).join(filename)
}

pub fn default_hf_cache_dir() -> anyhow::Result<PathBuf> {
    if let Some(hf_home) = env::var_os("HF_HOME") {
        return Ok(PathBuf::from(hf_home).join("hub"));
    }

    if let Some(home) = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")) {
        return Ok(PathBuf::from(home).join(".cache").join("huggingface").join("hub"));
    }

    bail!("unable to determine Hugging Face cache directory; use --cache-dir")
}

fn repo_cache_dir(cache_dir: &Path, repo_id: &str) -> PathBuf {
    cache_dir.join(format!("models--{}", repo_id.replace('/', "--")))
}

fn cached_file_candidate(repo_dir: &Path, path: PathBuf) -> Option<PathBuf> {
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        return None;
    }
    let canonical_repo = fs::canonicalize(repo_dir).ok()?;
    let canonical_target = fs::canonicalize(&path).ok()?;
    canonical_target.starts_with(&canonical_repo).then_some(path)
}

fn snapshot_candidate(repo_dir: &Path, revision: &str, filename: &str) -> Option<PathBuf> {
    let direct = repo_dir.join("snapshots").join(revision).join(filename);
    if let Some(candidate) = cached_file_candidate(repo_dir, direct) {
        return Some(candidate);
    }

    let ref_file = repo_dir.join("refs").join(revision);
    if let Ok(commit) = fs::read_to_string(ref_file) {
        let candidate = repo_dir.join("snapshots").join(commit.trim()).join(filename);
        if let Some(candidate) = cached_file_candidate(repo_dir, candidate) {
            return Some(candidate);
        }
    }
    None
}

pub fn resolve_hf_cached_file(
    cache_dir: &Path,
    repo_id: &str,
    revision: Option<&str>,
    filename: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let repo_dir = repo_cache_dir(cache_dir, repo_id);
    if !repo_dir.exists() {
        return Ok(None);
    }

    if let Some(revision) = revision {
        return Ok(snapshot_candidate(&repo_dir, revision, filename));
    }

    if let Some(main) = snapshot_candidate(&repo_dir, "main", filename) {
        return Ok(Some(main));
    }

    let snapshots = repo_dir.join("snapshots");
    if !snapshots.is_dir() {
        return Ok(None);
    }

    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(snapshots)? {
        let entry = entry?;
        let candidate = entry.path().join(filename);
        let Some(candidate) = cached_file_candidate(&repo_dir, candidate) else {
            continue;
        };
        let modified = fs::symlink_metadata(&candidate)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        if best.as_ref().is_none_or(|(current, _)| modified > *current) {
            best = Some((modified, candidate));
        }
    }
    Ok(best.map(|(_, path)| path))
}

pub fn huggingface_provenance(
    repo_id: &str,
    requested_revision: Option<&str>,
    resolved_path: &Path,
    filename: &str,
) -> crate::manifest::ArtifactProvenance {
    let mut parts = repo_id.splitn(2, '/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    let (namespace, model_name) = match second {
        Some(model) => (Some(first.to_string()), Some(model.to_string())),
        None => (None, Some(first.to_string())),
    };

    let resolved_revision = huggingface_snapshot_revision(resolved_path);
    let source_revision = resolved_revision
        .clone()
        .or_else(|| requested_revision.map(str::to_owned))
        .unwrap_or_else(|| "main".to_string());

    crate::manifest::ArtifactProvenance {
        provider: "huggingface".to_string(),
        namespace,
        repository: Some(repo_id.to_string()),
        model_name,
        requested_revision: Some(requested_revision.unwrap_or("main").to_string()),
        resolved_revision,
        filename: Some(filename.to_string()),
        source_uri: Some(format!("hf://{repo_id}@{source_revision}/{filename}")),
    }
}

pub fn huggingface_snapshot_revision(path: &Path) -> Option<String> {
    let mut saw_snapshots = false;
    for component in path.components() {
        if saw_snapshots {
            return component.as_os_str().to_str().map(ToOwned::to_owned);
        }
        saw_snapshots = component.as_os_str() == "snapshots";
    }
    None
}

pub fn download_hf_file(
    repo_id: &str,
    revision: Option<&str>,
    filename: &str,
    cache_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let mut cmd = Command::new("hf");
    cmd.arg("download")
        .arg(repo_id)
        .arg(filename)
        .arg("--cache-dir")
        .arg(cache_dir);
    if let Some(revision) = revision {
        cmd.arg("--revision").arg(revision);
    }

    let output = cmd.output().context(
        "failed to execute Hugging Face 'hf' CLI; install huggingface_hub or use --local-only with an existing cache entry",
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("hf download failed with status {}: {}", output.status, stderr.trim());
    }

    if let Some(path) = resolve_hf_cached_file(cache_dir, repo_id, revision, filename)? {
        return Ok(path);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().rev().map(str::trim).filter(|line| !line.is_empty()) {
        let path = PathBuf::from(line);
        if path.is_file() {
            return Ok(path);
        }
        if path.is_dir() {
            let candidate = path.join(filename);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    bail!("hf download completed but ModelVault could not locate {filename} in the cache")
}
