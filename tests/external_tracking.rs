use std::fs;

use modelvault::git_integration::{
    ensure_modelvault_gitignore, find_modelvault_pointers, pointer_path_for_source,
    source_is_inside_repo,
};
use tempfile::tempdir;

#[test]
fn external_source_gets_repository_local_pointer() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let external = tempdir()?;
    let source = external.path().join("model.safetensors");
    fs::write(&source, b"model")?;

    let id = "8087e9bf97c265f8435ed268733ecf3791825ad24850fd5d84d89e32ee3a589a";
    let pointer = pointer_path_for_source(repo.path(), &source, id, None)?;

    assert!(pointer.starts_with(repo.path()));
    assert_eq!(
        pointer
            .strip_prefix(repo.path())?
            .to_string_lossy()
            .replace('\\', "/"),
        "models/external/8087e9bf97c2-model.safetensors.mvptr"
    );
    assert!(!source_is_inside_repo(repo.path(), &source));
    Ok(())
}

#[test]
fn explicit_relative_pointer_is_resolved_from_git_root() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let external = tempdir()?;
    let source = external.path().join("model.safetensors");
    fs::write(&source, b"model")?;

    let pointer = pointer_path_for_source(
        repo.path(),
        &source,
        "abcd",
        Some(std::path::Path::new("models/all-MiniLM-L6-v2.mvptr")),
    )?;
    assert_eq!(pointer, repo.path().join("models/all-MiniLM-L6-v2.mvptr"));
    Ok(())
}

#[test]
fn external_source_is_not_written_to_gitignore() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let external = tempdir()?;
    let source = external.path().join("model.safetensors");
    fs::write(&source, b"model")?;

    ensure_modelvault_gitignore(repo.path(), &source)?;
    let ignore = fs::read_to_string(repo.path().join(".gitignore"))?;

    assert!(!ignore.contains(&source.to_string_lossy().replace('\\', "/")));
    assert!(ignore.contains(".modelvault/objects/"));
    assert!(ignore.contains(".modelvault/packs/"));
    assert!(ignore.contains(".modelvault/deltas/"));
    Ok(())
}

#[test]
fn broad_legacy_modelvault_ignore_is_migrated() -> anyhow::Result<()> {
    let repo = tempdir()?;
    let source = repo.path().join("models").join("model.safetensors");
    fs::create_dir_all(source.parent().unwrap())?;
    fs::write(&source, b"model")?;
    fs::write(
        repo.path().join(".gitignore"),
        "/target/\n.modelvault/\n*.tmp\n",
    )?;

    ensure_modelvault_gitignore(repo.path(), &source)?;
    let ignore = fs::read_to_string(repo.path().join(".gitignore"))?;

    assert!(!ignore.lines().any(|line| {
        matches!(
            line.trim(),
            ".modelvault" | ".modelvault/" | "/.modelvault" | "/.modelvault/"
        )
    }));
    assert!(ignore.contains(".modelvault/objects/"));
    assert!(ignore.contains(".modelvault/packs/"));
    assert!(ignore.contains(".modelvault/deltas/"));
    assert!(ignore.contains("*.tmp"));
    Ok(())
}

#[test]
fn pointer_discovery_ignores_git_and_modelvault_storage() -> anyhow::Result<()> {
    let repo = tempdir()?;
    fs::create_dir_all(repo.path().join("models"))?;
    fs::create_dir_all(repo.path().join(".git"))?;
    fs::create_dir_all(repo.path().join(".modelvault"))?;
    fs::write(repo.path().join("models/model.safetensors.mvptr"), "{}")?;
    fs::write(repo.path().join(".git/ignored.mvptr"), "{}")?;
    fs::write(repo.path().join(".modelvault/ignored.mvptr"), "{}")?;

    let pointers = find_modelvault_pointers(repo.path(), 10)?;
    assert_eq!(
        pointers,
        vec![std::path::PathBuf::from("models").join("model.safetensors.mvptr")]
    );
    Ok(())
}
