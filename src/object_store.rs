use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

#[cfg(feature = "s3")]
use anyhow::{ensure, Context};
#[cfg(feature = "s3")]
use aws_config::{BehaviorVersion, Region};
#[cfg(feature = "s3")]
use aws_sdk_s3::{
    config::retry::RetryConfig, operation::head_object::HeadObjectOutput, primitives::ByteStream,
    Client,
};
#[cfg(feature = "s3")]
use std::collections::HashSet;
#[cfg(feature = "s3")]
use std::fmt::Formatter;
#[cfg(feature = "s3")]
use tokio::runtime::Runtime;

use crate::{
    cas::{LocalCas, ObjectId, PutResult},
    config::RemoteDefinition,
    manifest::ArtifactManifest,
};

/// Transport-neutral interface for ModelVault content-addressed storage.
///
/// Implementations must preserve the invariant that an object is addressed by
/// the lowercase 64-character BLAKE3 digest of its exact bytes.
pub trait ObjectStore: Send + Sync + Debug {
    fn display_name(&self) -> String;
    fn contains(&self, id: &ObjectId) -> anyhow::Result<bool>;
    /// Fast verification suitable for normal synchronization. Backends may use
    /// trusted local hashes or remote metadata to avoid transferring the body.
    fn verify(&self, id: &ObjectId) -> anyhow::Result<bool>;
    /// Content verification that hashes the logical object bytes.
    fn verify_deep(&self, id: &ObjectId) -> anyhow::Result<bool> {
        Ok(ObjectId::from_bytes(&self.read(id)?) == *id)
    }
    fn read(&self, id: &ObjectId) -> anyhow::Result<Vec<u8>>;
    fn put_bytes(&self, bytes: &[u8]) -> anyhow::Result<PutResult>;
    fn remove(&self, id: &ObjectId) -> anyhow::Result<()>;
    fn save_manifest(&self, manifest: &ArtifactManifest) -> anyhow::Result<PathBuf>;
}

#[derive(Debug, Clone)]
pub struct FilesystemObjectStore {
    cas: LocalCas,
}

impl FilesystemObjectStore {
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        Ok(Self {
            cas: LocalCas::open(root)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.cas.root()
    }

    pub fn cas(&self) -> &LocalCas {
        &self.cas
    }
}

impl ObjectStore for FilesystemObjectStore {
    fn display_name(&self) -> String {
        self.cas.root().display().to_string()
    }

    fn contains(&self, id: &ObjectId) -> anyhow::Result<bool> {
        Ok(self.cas.contains(id))
    }

    fn verify(&self, id: &ObjectId) -> anyhow::Result<bool> {
        Ok(self.cas.verify(id).unwrap_or(false))
    }

    fn read(&self, id: &ObjectId) -> anyhow::Result<Vec<u8>> {
        Ok(self.cas.read(id)?)
    }

    fn put_bytes(&self, bytes: &[u8]) -> anyhow::Result<PutResult> {
        Ok(self.cas.put_bytes(bytes)?)
    }

    fn remove(&self, id: &ObjectId) -> anyhow::Result<()> {
        self.cas.remove_unpacked_object(id)?;
        Ok(())
    }

    fn save_manifest(&self, manifest: &ArtifactManifest) -> anyhow::Result<PathBuf> {
        Ok(manifest.save(self.cas.root())?)
    }
}

#[cfg(feature = "s3")]
pub struct S3ObjectStore {
    client: Client,
    runtime: Runtime,
    bucket: String,
    prefix: String,
    endpoint: Option<String>,
}

#[cfg(feature = "s3")]
#[derive(Debug, Clone, Default)]
pub struct S3AuditReport {
    pub manifests: usize,
    pub manifest_errors: Vec<String>,
    pub referenced_objects: usize,
    pub missing_objects: usize,
    pub corrupt_objects: usize,
    pub object_count: usize,
    pub physical_bytes: u64,
    pub logical_bytes: u64,
    pub orphan_objects: usize,
    pub orphan_bytes: u64,
    pub removed_objects: usize,
    pub removed_bytes: u64,
}

#[cfg(feature = "s3")]
impl Debug for S3ObjectStore {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3ObjectStore")
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "s3")]
impl S3ObjectStore {
    pub fn open(definition: &RemoteDefinition) -> anyhow::Result<Self> {
        ensure!(
            definition.kind == "s3",
            "S3ObjectStore requires an S3 remote definition"
        );
        let bucket = definition.bucket()?.to_string();
        let prefix = definition
            .prefix
            .as_deref()
            .unwrap_or_default()
            .trim_matches('/')
            .to_string();
        let endpoint = definition.endpoint.clone();
        let runtime = Runtime::new().context("failed to initialize Tokio runtime for S3")?;

        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(region) = &definition.region {
            loader = loader.region(Region::new(region.clone()));
        }
        if let Some(profile) = &definition.profile {
            loader = loader.profile_name(profile);
        }
        let shared = runtime.block_on(loader.load());

        let retry = RetryConfig::standard().with_max_attempts(definition.max_attempts);
        let mut builder = aws_sdk_s3::config::Builder::from(&shared)
            .retry_config(retry)
            .force_path_style(definition.force_path_style);
        if let Some(url) = &definition.endpoint {
            builder = builder.endpoint_url(url.clone());
        }
        let client = Client::from_conf(builder.build());

        Ok(Self {
            client,
            runtime,
            bucket,
            prefix,
            endpoint,
        })
    }

    fn key(&self, suffix: &str) -> String {
        if self.prefix.is_empty() {
            suffix.to_string()
        } else {
            format!("{}/{}", self.prefix, suffix)
        }
    }

    fn object_key(&self, id: &ObjectId) -> String {
        let value = id.as_str();
        self.key(&format!("objects/{}/{}", &value[..2], &value[2..]))
    }

    fn manifest_key(&self, manifest: &ArtifactManifest) -> String {
        self.key(&format!("manifests/{}.json", manifest.artifact_id))
    }

    fn head_object(&self, id: &ObjectId) -> anyhow::Result<Option<HeadObjectOutput>> {
        let key = self.object_key(id);
        let result = self.runtime.block_on(
            self.client
                .head_object()
                .bucket(&self.bucket)
                .key(&key)
                .send(),
        );
        match result {
            Ok(output) => Ok(Some(output)),
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                Ok(None)
            }
            Err(error) => Err(anyhow::anyhow!(error))
                .with_context(|| format!("failed to inspect s3://{}/{}", self.bucket, key)),
        }
    }

    fn put_key(
        &self,
        key: &str,
        bytes: Vec<u8>,
        blake3_id: Option<&ObjectId>,
    ) -> anyhow::Result<()> {
        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes));
        if let Some(id) = blake3_id {
            request = request.metadata("modelvault-blake3", id.as_str());
        }
        self.runtime
            .block_on(request.send())
            .with_context(|| format!("failed to put s3://{}/{}", self.bucket, key))?;
        Ok(())
    }

    fn list_prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, u64)>> {
        const MAX_LISTED: usize = 100_000;
        let mut continuation = None;
        let mut listed = Vec::new();
        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(token) = continuation.as_deref() {
                request = request.continuation_token(token);
            }
            let page = self
                .runtime
                .block_on(request.send())
                .with_context(|| format!("failed to list s3://{}/{}", self.bucket, prefix))?;
            for item in page.contents() {
                let key = item
                    .key()
                    .context("S3 listing returned an object without a key")?;
                let size = u64::try_from(item.size().unwrap_or(0))
                    .context("S3 object size cannot be negative")?;
                listed.push((key.to_string(), size));
                ensure!(
                    listed.len() <= MAX_LISTED,
                    "S3 remote listing exceeds safety limit of {MAX_LISTED} objects"
                );
            }
            if !page.is_truncated().unwrap_or(false) {
                break;
            }
            continuation = page.next_continuation_token().map(ToOwned::to_owned);
            ensure!(
                continuation.is_some(),
                "S3 listing is truncated without a continuation token"
            );
        }
        Ok(listed)
    }

    fn read_key(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let output = self
            .runtime
            .block_on(
                self.client
                    .get_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .send(),
            )
            .with_context(|| format!("failed to get s3://{}/{}", self.bucket, key))?;
        Ok(self.runtime.block_on(output.body.collect())?.to_vec())
    }

    pub fn audit(&self, deep: bool, prune: bool) -> anyhow::Result<S3AuditReport> {
        let manifest_prefix = self.key("manifests/");
        let object_prefix = self.key("objects/");
        let mut report = S3AuditReport::default();
        let mut manifests = Vec::new();
        for (key, _) in self.list_prefix(&manifest_prefix)? {
            let Some(name) = key.strip_prefix(&manifest_prefix) else {
                continue;
            };
            if name.len() != 69
                || !name.ends_with(".json")
                || !name[..64].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                continue;
            }
            report.manifests += 1;
            match serde_json::from_slice::<ArtifactManifest>(&self.read_key(&key)?) {
                Ok(manifest) if manifest.artifact_id.eq_ignore_ascii_case(&name[..64]) => {
                    match crate::manifest::validate_manifest_structure(&manifest) {
                        Ok(()) => {
                            report.logical_bytes =
                                report.logical_bytes.saturating_add(manifest.logical_size);
                            manifests.push(manifest);
                        }
                        Err(error) => report.manifest_errors.push(format!("{key}: {error}")),
                    }
                }
                Ok(_) => report.manifest_errors.push(format!(
                    "{key}: manifest artifact ID does not match canonical key"
                )),
                Err(error) => report
                    .manifest_errors
                    .push(format!("{key}: invalid manifest: {error}")),
            }
        }
        let refs: HashSet<String> = manifests
            .iter()
            .flat_map(|manifest| manifest.chunks.iter().map(|chunk| chunk.object.clone()))
            .collect();
        report.referenced_objects = refs.len();
        for value in &refs {
            let id = ObjectId::parse(value)?;
            if !self.contains(&id)? {
                report.missing_objects += 1;
            } else if !(if deep {
                self.verify_deep(&id)?
            } else {
                self.verify(&id)?
            }) {
                report.corrupt_objects += 1;
            }
        }
        let mut objects = Vec::new();
        for (key, size) in self.list_prefix(&object_prefix)? {
            let Some(suffix) = key.strip_prefix(&object_prefix) else {
                continue;
            };
            let compact = suffix.replace('/', "");
            let Ok(id) = ObjectId::parse(&compact) else {
                continue;
            };
            if self.object_key(&id) == key {
                objects.push((id, size));
            }
        }
        report.object_count = objects.len();
        report.physical_bytes = objects.iter().map(|(_, size)| *size).sum();
        for (id, size) in objects {
            if refs.contains(id.as_str()) {
                continue;
            }
            report.orphan_objects += 1;
            report.orphan_bytes = report.orphan_bytes.saturating_add(size);
            if prune {
                self.remove(&id)?;
                report.removed_objects += 1;
                report.removed_bytes = report.removed_bytes.saturating_add(size);
            }
        }
        Ok(report)
    }
}

#[cfg(feature = "s3")]
impl ObjectStore for S3ObjectStore {
    fn display_name(&self) -> String {
        match &self.endpoint {
            Some(endpoint) => format!("s3://{} (endpoint {})", self.bucket, endpoint),
            None => format!("s3://{}", self.bucket),
        }
    }

    fn contains(&self, id: &ObjectId) -> anyhow::Result<bool> {
        Ok(self.head_object(id)?.is_some())
    }

    fn verify(&self, id: &ObjectId) -> anyhow::Result<bool> {
        let Some(head) = self.head_object(id)? else {
            return Ok(false);
        };
        if head
            .metadata()
            .and_then(|metadata| metadata.get("modelvault-blake3"))
            .is_some_and(|stored| stored == id.as_str())
        {
            return Ok(true);
        }
        // Compatibility fallback for objects created without ModelVault metadata.
        Ok(ObjectId::from_bytes(&self.read(id)?) == *id)
    }

    fn read(&self, id: &ObjectId) -> anyhow::Result<Vec<u8>> {
        let key = self.object_key(id);
        let output = self
            .runtime
            .block_on(
                self.client
                    .get_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send(),
            )
            .with_context(|| format!("failed to get s3://{}/{}", self.bucket, key))?;
        let collected = self
            .runtime
            .block_on(output.body.collect())
            .with_context(|| {
                format!(
                    "failed to read response body for s3://{}/{}",
                    self.bucket, key
                )
            })?;
        Ok(collected.to_vec())
    }

    fn put_bytes(&self, bytes: &[u8]) -> anyhow::Result<PutResult> {
        let id = ObjectId::from_bytes(bytes);
        let size = bytes.len() as u64;
        if self.contains(&id)? && self.verify(&id)? {
            return Ok(PutResult {
                id,
                size,
                was_new: false,
            });
        }
        if self.contains(&id)? {
            self.remove(&id)?;
        }
        let key = self.object_key(&id);
        self.put_key(&key, bytes.to_vec(), Some(&id))?;
        ensure!(
            self.verify(&id)?,
            "S3 object {} failed verification after upload",
            id
        );
        Ok(PutResult {
            id,
            size,
            was_new: true,
        })
    }

    fn remove(&self, id: &ObjectId) -> anyhow::Result<()> {
        let key = self.object_key(id);
        self.runtime
            .block_on(
                self.client
                    .delete_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send(),
            )
            .with_context(|| format!("failed to delete s3://{}/{}", self.bucket, key))?;
        Ok(())
    }

    fn save_manifest(&self, manifest: &ArtifactManifest) -> anyhow::Result<PathBuf> {
        let key = self.manifest_key(manifest);
        let bytes = serde_json::to_vec_pretty(manifest)?;
        self.put_key(&key, bytes, None)?;
        Ok(PathBuf::from(format!("s3://{}/{}", self.bucket, key)))
    }
}

pub fn open_remote_store(definition: &RemoteDefinition) -> anyhow::Result<Box<dyn ObjectStore>> {
    match definition.kind.as_str() {
        "filesystem" => Ok(Box::new(FilesystemObjectStore::open(
            definition.filesystem_path()?.to_path_buf(),
        )?)),
        "s3" => {
            #[cfg(feature = "s3")]
            {
                Ok(Box::new(S3ObjectStore::open(definition)?))
            }
            #[cfg(not(feature = "s3"))]
            {
                anyhow::bail!(
                    "S3 support is not compiled in; rebuild ModelVault with '--features s3'"
                )
            }
        }
        other => anyhow::bail!("unsupported remote type '{other}'"),
    }
}
