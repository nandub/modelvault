use std::{fs, path::Path};

use modelvault::import::{
    default_hf_target, pointer_path_for_target, repository_target_path, resolve_hf_cached_file,
};
use tempfile::tempdir;

#[test]
fn import_target_is_repository_local_and_pointer_matches_logical_name() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let target = repository_target_path(
        repo.path(),
        Path::new("models/all-MiniLM-L6-v2/model.safetensors"),
    )?;
    assert_eq!(
        target,
        repo.path()
            .join("models/all-MiniLM-L6-v2/model.safetensors")
    );
    assert_eq!(
        pointer_path_for_target(&target),
        repo.path()
            .join("models/all-MiniLM-L6-v2/model.safetensors.mvptr")
    );
    #[cfg(windows)]
    assert!(
        !target.to_string_lossy().starts_with(r"\\?\"),
        "repository-local import paths must not expose the Windows verbatim-path prefix"
    );
    Ok(())
}

#[test]
fn import_target_rejects_parent_traversal() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let err =
        repository_target_path(repo.path(), Path::new("../outside/model.safetensors")).unwrap_err();
    assert!(err.to_string().contains("repository-relative"));
    Ok(())
}

#[test]
fn huggingface_cache_resolves_refs_main() -> anyhow::Result<()> {
    let cache = tempdir()?;
    let repo = cache
        .path()
        .join("models--sentence-transformers--all-MiniLM-L6-v2");
    let snapshot = repo.join("snapshots").join("abc123");
    fs::create_dir_all(&snapshot)?;
    fs::create_dir_all(repo.join("refs"))?;
    fs::write(repo.join("refs").join("main"), "abc123\n")?;
    fs::write(snapshot.join("model.safetensors"), b"weights")?;

    let resolved = resolve_hf_cached_file(
        cache.path(),
        "sentence-transformers/all-MiniLM-L6-v2",
        Some("main"),
        "model.safetensors",
    )?
    .expect("cached file");

    assert_eq!(resolved, snapshot.join("model.safetensors"));
    Ok(())
}

#[test]
fn huggingface_cache_falls_back_to_available_snapshot() -> anyhow::Result<()> {
    let cache = tempdir()?;
    let repo = cache.path().join("models--org--demo");
    let snapshot = repo.join("snapshots").join("commit1");
    fs::create_dir_all(&snapshot)?;
    fs::write(snapshot.join("weights.safetensors"), b"weights")?;

    let resolved = resolve_hf_cached_file(cache.path(), "org/demo", None, "weights.safetensors")?
        .expect("cached file");
    assert_eq!(resolved, snapshot.join("weights.safetensors"));
    Ok(())
}

#[test]
fn huggingface_default_target_is_repository_friendly() {
    assert_eq!(
        default_hf_target(
            "sentence-transformers/all-MiniLM-L6-v2",
            "model.safetensors"
        ),
        Path::new("models/all-MiniLM-L6-v2/model.safetensors")
    );
}

#[test]
fn rejected_absolute_target_does_not_create_parent_directories() -> anyhow::Result<()> {
    let workspace = tempdir()?;
    let repo = workspace.path().join("repo");
    fs::create_dir_all(&repo)?;
    let outside_parent = workspace.path().join("outside").join("nested");
    let target = outside_parent.join("model.safetensors");
    let err = repository_target_path(&repo, &target).unwrap_err();
    assert!(err.to_string().contains("inside the Git repository"));
    assert!(!outside_parent.exists());
    Ok(())
}
