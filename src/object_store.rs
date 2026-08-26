use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

#[cfg(feature = "s3")]
use std::fmt::Formatter;
#[cfg(feature = "s3")]
use anyhow::{ensure, Context};
#[cfg(feature = "s3")]
use aws_config::{BehaviorVersion, Region};
#[cfg(feature = "s3")]
use aws_sdk_s3::{
    config::retry::RetryConfig,
    operation::head_object::HeadObjectOutput,
    primitives::ByteStream,
    Client,
};
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
        Ok(Self { cas: LocalCas::open(root)? })
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
        ensure!(definition.kind == "s3", "S3ObjectStore requires an S3 remote definition");
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
            Err(error) if error.raw_response().is_some_and(|response| response.status().as_u16() == 404) => Ok(None),
            Err(error) => Err(anyhow::anyhow!(error))
                .with_context(|| format!("failed to inspect s3://{}/{}", self.bucket, key)),
        }
    }

    fn put_key(&self, key: &str, bytes: Vec<u8>, blake3_id: Option<&ObjectId>) -> anyhow::Result<()> {
        let mut request = self.client
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
            .with_context(|| format!("failed to read response body for s3://{}/{}", self.bucket, key))?;
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
        ensure!(self.verify(&id)?, "S3 object {} failed verification after upload", id);
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
