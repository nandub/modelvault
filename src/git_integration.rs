use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, ensure, Context};

pub fn git_root() -> anyhow::Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to execute git")?;
    if !output.status.success() {
        bail!("current directory is not inside a Git work tree");
    }
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn source_is_inside_repo(root: &Path, source: &Path) -> bool {
    canonical_or_original(source).starts_with(canonical_or_original(root))
}

pub fn pointer_path_for_source(
    root: &Path,
    source: &Path,
    artifact_id: &str,
    requested: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    if let Some(requested) = requested {
        return Ok(if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            root.join(requested)
        });
    }

    if source_is_inside_repo(root, source) {
        return Ok(crate::pointer::ArtifactPointer::default_pointer_path(source));
    }

    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let short_id: String = artifact_id.chars().take(12).collect();
    Ok(root
        .join("models")
        .join("external")
        .join(format!("{short_id}-{source_name}.mvptr")))
}

pub fn ensure_modelvault_gitignore(root: &Path, source: &Path) -> anyhow::Result<()> {
    let ignore = root.join(".gitignore");
    let original = if ignore.exists() {
        fs::read_to_string(&ignore)?
    } else {
        String::new()
    };

    // Older ModelVault packages used a broad `.modelvault/` rule. That also hides
    // Git-tracked manifests, so migrate only those exact legacy root-level rules
    // to the narrower physical-storage rules below. Do not rewrite arbitrary
    // user patterns such as `.modelvault/cache-*`.
    let legacy_rules = [".modelvault", ".modelvault/", "/.modelvault", "/.modelvault/"];
    let mut lines: Vec<String> = original
        .lines()
        .filter(|line| !legacy_rules.contains(&line.trim()))
        .map(ToOwned::to_owned)
        .collect();

    let mut required = vec![
        ".modelvault/objects/".to_string(),
        ".modelvault/tmp/".to_string(),
        ".modelvault/packs/".to_string(),
        ".modelvault/deltas/".to_string(),
    ];

    if source_is_inside_repo(root, source) {
        let source_abs = canonical_or_original(source);
        let root_abs = canonical_or_original(root);
        if let Ok(rel) = source_abs.strip_prefix(&root_abs) {
            required.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }

    for rule in required {
        if !lines.iter().any(|existing| existing.trim() == rule) {
            lines.push(rule);
        }
    }

    let mut updated = lines.join("\n");
    if !updated.is_empty() {
        updated.push('\n');
    }

    if updated != original {
        let mut f = fs::File::create(ignore)?;
        f.write_all(updated.as_bytes())?;
    }
    Ok(())
}

pub fn git_add(paths: &[PathBuf]) -> anyhow::Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("add").arg("--");
    for p in paths {
        cmd.arg(p);
    }
    let status = cmd.status().context("failed to execute git add")?;
    if !status.success() {
        bail!("git add failed with status {status}");
    }
    Ok(())
}


pub fn git_add_force(paths: &[PathBuf]) -> anyhow::Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("add").arg("-f").arg("--");
    for p in paths {
        cmd.arg(p);
    }
    let status = cmd.status().context("failed to execute git add -f")?;
    if !status.success() {
        bail!("git add -f failed with status {status}");
    }
    Ok(())
}

pub fn install_modelvault_post_checkout_hook(root: &Path, force: bool) -> anyhow::Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", "hooks"])
        .current_dir(root)
        .output()
        .context("failed to locate Git hooks directory")?;
    ensure!(output.status.success(), "unable to locate Git hooks directory");
    let hooks = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    let hooks = if hooks.is_absolute() { hooks } else { root.join(hooks) };
    let hook = hooks.join("post-checkout");
    if hook.exists() && !force {
        bail!("Git hook already exists at {}; use --force only if it is safe to replace", hook.display());
    }
    fs::create_dir_all(&hooks)?;
    fs::write(&hook, "#!/bin/sh\n# Installed by ModelVault; this hook performs no network or filesystem materialization.\nif command -v modelvault >/dev/null 2>&1; then\n  modelvault checkout-advice\nfi\n")?;
    Ok(hook)
}

pub fn find_modelvault_pointers(root: &Path, limit: usize) -> anyhow::Result<Vec<PathBuf>> {
    fn visit(root: &Path, current: &Path, found: &mut Vec<PathBuf>, limit: usize) -> anyhow::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                let name = entry.file_name();
                if name == ".git" || name == ".modelvault" { continue; }
                visit(root, &path, found, limit)?;
            } else if ty.is_file() && path.extension().and_then(|value| value.to_str()).is_some_and(|value| value.eq_ignore_ascii_case("mvptr")) {
                found.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
                ensure!(found.len() <= limit, "too many .mvptr files; limit is {limit}");
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    visit(root, root, &mut found, limit)?;
    found.sort();
    Ok(found)
}
