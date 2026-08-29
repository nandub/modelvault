use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub force_path_style: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_max_attempts() -> u32 {
    4
}

impl RemoteDefinition {
    pub fn filesystem(path: PathBuf) -> Self {
        Self {
            kind: "filesystem".to_string(),
            path: Some(path),
            bucket: None,
            prefix: None,
            region: None,
            endpoint: None,
            force_path_style: false,
            profile: None,
            max_attempts: default_max_attempts(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn s3(
        bucket: String,
        prefix: Option<String>,
        region: Option<String>,
        endpoint: Option<String>,
        force_path_style: bool,
        profile: Option<String>,
        max_attempts: u32,
    ) -> anyhow::Result<Self> {
        ensure!(!bucket.trim().is_empty(), "S3 bucket cannot be empty");
        ensure!(
            max_attempts > 0,
            "S3 max attempts must be greater than zero"
        );
        Ok(Self {
            kind: "s3".to_string(),
            path: None,
            bucket: Some(bucket),
            prefix: prefix
                .map(|p| normalize_prefix(&p))
                .filter(|p| !p.is_empty()),
            region,
            endpoint,
            force_path_style,
            profile,
            max_attempts,
        })
    }

    pub fn filesystem_path(&self) -> anyhow::Result<&Path> {
        ensure!(self.kind == "filesystem", "remote is not filesystem-backed");
        self.path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("filesystem remote is missing 'path'"))
    }

    pub fn bucket(&self) -> anyhow::Result<&str> {
        ensure!(self.kind == "s3", "remote is not S3-backed");
        self.bucket
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("S3 remote is missing 'bucket'"))
    }
}

fn normalize_prefix(prefix: &str) -> String {
    prefix.trim_matches('/').replace('\\', "/")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ModelVaultConfig {
    #[serde(default)]
    pub default_remote: Option<String>,
    #[serde(default)]
    pub remotes: BTreeMap<String, RemoteDefinition>,
}

impl ModelVaultConfig {
    pub fn path(store_root: &Path) -> PathBuf {
        store_root.join(CONFIG_FILE)
    }

    pub fn load(store_root: &Path) -> anyhow::Result<Self> {
        let path = Self::path(store_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn save(&self, store_root: &Path) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(store_root)?;
        let path = Self::path(store_root);
        let bytes = serde_json::to_vec_pretty(self)?;
        fs::write(&path, bytes)?;
        Ok(path)
    }

    pub fn add_filesystem_remote(&mut self, name: &str, path: PathBuf) -> anyhow::Result<()> {
        self.add_remote(name, RemoteDefinition::filesystem(path))
    }

    pub fn add_s3_remote(&mut self, name: &str, remote: RemoteDefinition) -> anyhow::Result<()> {
        ensure!(remote.kind == "s3", "remote definition must have type 's3'");
        self.add_remote(name, remote)
    }

    fn add_remote(&mut self, name: &str, remote: RemoteDefinition) -> anyhow::Result<()> {
        validate_remote_name(name)?;
        ensure!(
            !self.remotes.contains_key(name),
            "remote '{name}' already exists"
        );
        self.remotes.insert(name.to_string(), remote);
        Ok(())
    }

    pub fn remove_remote(&mut self, name: &str) -> anyhow::Result<()> {
        ensure!(
            self.remotes.remove(name).is_some(),
            "remote '{name}' does not exist"
        );
        if self.default_remote.as_deref() == Some(name) {
            self.default_remote = None;
        }
        Ok(())
    }

    pub fn set_default(&mut self, name: &str) -> anyhow::Result<()> {
        ensure!(
            self.remotes.contains_key(name),
            "remote '{name}' does not exist"
        );
        self.default_remote = Some(name.to_string());
        Ok(())
    }

    pub fn resolve(&self, selector: Option<&str>) -> anyhow::Result<(String, RemoteDefinition)> {
        let name = selector
            .map(str::to_string)
            .or_else(|| self.default_remote.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("no remote specified and no default remote is configured")
            })?;
        let remote = self
            .remotes
            .get(&name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("remote '{name}' is not configured"))?;
        Ok((name, remote))
    }
}

fn validate_remote_name(name: &str) -> anyhow::Result<()> {
    ensure!(!name.is_empty(), "remote name cannot be empty");
    ensure!(
        name.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "remote name may contain only ASCII letters, digits, '-', '_' and '.'"
    );
    Ok(())
}
