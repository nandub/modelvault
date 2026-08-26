use std::{
    collections::HashSet,
    fmt,
    fs,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const COMPRESSED_MAGIC: &[u8; 4] = b"MVZ1";
const DELTA_MAGIC: &[u8; 4] = b"MVD1";
const DELTA_HEADER_LEN: usize = 4 + 64 + 1 + 8;
const REPOSITORY_FILE: &str = "repository.json";
const PACK_VERSION: u32 = 2;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn create_temp_file_near(target: &Path) -> io::Result<(PathBuf, fs::File)> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let stem = target.file_name().and_then(|value| value.to_str()).unwrap_or("modelvault");
    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = parent.join(format!(
            ".{stem}.tmp-{}-{nanos:x}-{sequence:x}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to create a unique ModelVault temporary file",
    ))
}

#[cfg(feature = "compression")]
fn decode_zstd_bounded<R: Read>(reader: R, expected: u64, label: &str) -> io::Result<Vec<u8>> {
    let decoder = zstd::stream::read::Decoder::new(reader)?;
    let limit = expected.checked_add(1).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{label} declares an unsupported logical size"))
    })?;
    let mut bounded = decoder.take(limit);
    let mut decoded = Vec::new();
    bounded.read_to_end(&mut decoded)?;
    if decoded.len() as u64 != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} decoded to {} bytes, expected {expected}", decoded.len()),
        ));
    }
    Ok(decoded)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    pub fn parse(value: &str) -> io::Result<Self> {
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "object id must be a 64-character hexadecimal BLAKE3 digest",
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutResult {
    pub id: ObjectId,
    pub size: u64,
    pub was_new: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionMode {
    None,
    Zstd,
}

impl fmt::Display for CompressionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Zstd => write!(f, "zstd"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryMetadata {
    pub version: u32,
    pub object_hash: String,
    pub loose_compression: CompressionMode,
    pub zstd_level: i32,
    pub pack_format_version: u32,
    #[serde(default = "default_delta_min_savings_pct")]
    pub delta_min_savings_pct: u8,
    #[serde(default = "default_max_delta_depth")]
    pub max_delta_depth: u8,
}

fn default_delta_min_savings_pct() -> u8 { 20 }
fn default_max_delta_depth() -> u8 { 2 }

impl Default for RepositoryMetadata {
    fn default() -> Self {
        Self {
            version: 1,
            object_hash: "blake3".to_string(),
            loose_compression: CompressionMode::None,
            zstd_level: 3,
            pack_format_version: PACK_VERSION,
            delta_min_savings_pct: default_delta_min_savings_pct(),
            max_delta_depth: default_max_delta_depth(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackIndex {
    version: u32,
    pack_file: String,
    objects: Vec<PackEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum PackEncoding {
    #[default]
    Raw,
    Zstd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackEntry {
    id: String,
    offset: u64,
    /// Logical, decoded byte length. Kept as `size` for v1 index compatibility.
    size: u64,
    #[serde(default)]
    stored_size: u64,
    #[serde(default)]
    encoding: PackEncoding,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhysicalStorageBreakdown {
    pub loose_raw_bytes: u64,
    pub loose_compressed_bytes: u64,
    pub delta_bytes: u64,
    pub pack_data_bytes: u64,
    pub pack_index_bytes: u64,
    pub manifest_bytes: u64,
    pub metadata_bytes: u64,
}

impl PhysicalStorageBreakdown {
    pub fn total(&self) -> u64 {
        self.loose_raw_bytes
            .saturating_add(self.loose_compressed_bytes)
            .saturating_add(self.delta_bytes)
            .saturating_add(self.pack_data_bytes)
            .saturating_add(self.pack_index_bytes)
            .saturating_add(self.manifest_bytes)
            .saturating_add(self.metadata_bytes)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptimizeReport {
    pub dry_run: bool,
    pub objects_considered: usize,
    pub objects_packed: usize,
    pub deltas_retained: usize,
    pub before_bytes: u64,
    pub estimated_after_bytes: u64,
    pub old_pack_files_removed: usize,
    pub loose_removed: usize,
    pub deltas_removed: usize,
    pub pack_path: Option<PathBuf>,
    pub index_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompressionMigrationReport {
    pub objects_rewritten: usize,
    pub logical_bytes: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepackReport {
    pub objects_packed: usize,
    pub logical_bytes: u64,
    pub pack_bytes: u64,
    pub loose_removed: usize,
    pub pack_path: Option<PathBuf>,
    pub index_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackVerifyReport {
    pub packs_scanned: usize,
    pub objects_verified: usize,
    pub logical_bytes: u64,
    pub errors: Vec<String>,
}

impl PackVerifyReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackCompactReport {
    pub objects_packed: usize,
    pub logical_bytes: u64,
    pub pack_bytes: u64,
    pub old_pack_files_removed: usize,
    pub loose_removed: usize,
    pub pack_path: Option<PathBuf>,
    pub index_path: Option<PathBuf>,
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaObjectInfo {
    pub base: ObjectId,
    pub depth: u8,
    pub logical_size: u64,
    pub physical_size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeltaInstallResult {
    pub stored: bool,
    pub full_physical_bytes: u64,
    pub delta_physical_bytes: u64,
    pub savings_bytes: u64,
    pub savings_pct: f64,
    pub depth: u8,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LocalCas {
    root: PathBuf,
    metadata: RepositoryMetadata,
}

impl LocalCas {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("packs"))?;
        fs::create_dir_all(root.join("deltas"))?;
        let metadata_path = root.join(REPOSITORY_FILE);
        let metadata = if metadata_path.is_file() {
            let bytes = fs::read(&metadata_path)?;
            let mut parsed: RepositoryMetadata = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Self::validate_metadata(&parsed)?;
            if parsed.pack_format_version == 1 {
                parsed.pack_format_version = PACK_VERSION;
                Self::write_metadata(&root, &parsed)?;
            }
            parsed
        } else {
            let metadata = RepositoryMetadata::default();
            Self::write_metadata(&root, &metadata)?;
            metadata
        };
        Ok(Self { root, metadata })
    }

    fn validate_metadata(metadata: &RepositoryMetadata) -> io::Result<()> {
        if metadata.version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported ModelVault repository version {}", metadata.version),
            ));
        }
        if metadata.object_hash != "blake3" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported object hash '{}'", metadata.object_hash),
            ));
        }
        if metadata.pack_format_version != 1 && metadata.pack_format_version != PACK_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported pack format version {}", metadata.pack_format_version),
            ));
        }
        Ok(())
    }

    fn write_metadata(root: &Path, metadata: &RepositoryMetadata) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(metadata)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::atomic_write(&root.join(REPOSITORY_FILE), &bytes)
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let (tmp, mut output) = create_temp_file_near(path)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        if !path.exists() {
            return match fs::rename(&tmp, path) {
                Ok(()) => Ok(()),
                Err(err) => {
                    let _ = fs::remove_file(&tmp);
                    Err(err)
                }
            };
        }

        let backup_sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let backup_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let backup = path.with_extension(format!(
            "bak-{}-{backup_nanos:x}-{backup_sequence:x}",
            std::process::id()
        ));
        fs::rename(path, &backup)?;
        match fs::rename(&tmp, path) {
            Ok(()) => {
                let _ = fs::remove_file(&backup);
                Ok(())
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                let _ = fs::rename(&backup, path);
                Err(err)
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn metadata(&self) -> &RepositoryMetadata {
        &self.metadata
    }

    pub fn set_compression(&mut self, mode: CompressionMode, zstd_level: i32) -> io::Result<()> {
        self.metadata.loose_compression = mode;
        self.metadata.zstd_level = zstd_level;
        Self::write_metadata(&self.root, &self.metadata)
    }

    pub fn set_delta_policy(&mut self, min_savings_pct: u8, max_depth: u8) -> io::Result<()> {
        if min_savings_pct > 100 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "delta minimum savings percentage must be <= 100"));
        }
        if max_depth == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "maximum delta depth must be greater than zero"));
        }
        self.metadata.delta_min_savings_pct = min_savings_pct;
        self.metadata.max_delta_depth = max_depth;
        Self::write_metadata(&self.root, &self.metadata)
    }

    pub fn object_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.as_str();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    pub fn delta_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.as_str();
        self.root.join("deltas").join(&hex[..2]).join(format!("{}.mvdelta", &hex[2..]))
    }

    fn delta_contains(&self, id: &ObjectId) -> bool {
        self.delta_path(id).is_file()
    }

    fn loose_contains(&self, id: &ObjectId) -> bool {
        self.object_path(id).is_file()
    }

    pub fn contains(&self, id: &ObjectId) -> bool {
        self.loose_contains(id) || self.delta_contains(id) || self.find_pack_entry(id).ok().flatten().is_some()
    }

    fn encode_loose(&self, bytes: &[u8]) -> io::Result<Vec<u8>> {
        match self.metadata.loose_compression {
            CompressionMode::None => Ok(bytes.to_vec()),
            CompressionMode::Zstd => {
                #[cfg(feature = "compression")]
                {
                    let compressed = zstd::stream::encode_all(std::io::Cursor::new(bytes), self.metadata.zstd_level)?;
                    let mut encoded = Vec::with_capacity(12 + compressed.len());
                    encoded.extend_from_slice(COMPRESSED_MAGIC);
                    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                    encoded.extend_from_slice(&compressed);
                    Ok(encoded)
                }
                #[cfg(not(feature = "compression"))]
                {
                    let _ = bytes;
                    Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "repository uses zstd compression; rebuild ModelVault with '--features compression'",
                    ))
                }
            }
        }
    }

    fn decode_loose_bytes(encoded: &[u8]) -> io::Result<Vec<u8>> {
        if !encoded.starts_with(COMPRESSED_MAGIC) {
            return Ok(encoded.to_vec());
        }
        if encoded.len() < 12 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated ModelVault compressed object header"));
        }
        let expected = u64::from_le_bytes(encoded[4..12].try_into().expect("slice length checked"));
        #[cfg(feature = "compression")]
        {
            decode_zstd_bounded(
                std::io::Cursor::new(&encoded[12..]),
                expected,
                "compressed object",
            )
        }
        #[cfg(not(feature = "compression"))]
        {
            let _ = expected;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "compressed CAS object requires the 'compression' feature",
            ))
        }
    }

    pub fn put_bytes(&self, bytes: &[u8]) -> io::Result<PutResult> {
        let id = ObjectId::from_bytes(bytes);
        let target = self.object_path(&id);

        if self.contains(&id) && self.verify(&id).unwrap_or(false) {
            return Ok(PutResult { id, size: bytes.len() as u64, was_new: false });
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        let encoded = self.encode_loose(bytes)?;
        let (tmp, mut output) = create_temp_file_near(&target)?;
        output.write_all(&encoded)?;
        output.sync_all()?;
        drop(output);

        match fs::rename(&tmp, &target) {
            Ok(()) => {}
            Err(err) if target.is_file() => {
                let _ = fs::remove_file(&tmp);
                if err.kind() != io::ErrorKind::AlreadyExists {
                    // Another writer may have won the race; target existence is sufficient.
                }
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
        }

        Ok(PutResult { id, size: bytes.len() as u64, was_new: true })
    }

    pub fn read(&self, id: &ObjectId) -> io::Result<Vec<u8>> {
        let mut visiting = HashSet::new();
        self.read_internal(id, &mut visiting)
    }

    fn read_internal(&self, id: &ObjectId, visiting: &mut HashSet<String>) -> io::Result<Vec<u8>> {
        let loose = self.object_path(id);
        if loose.is_file() {
            return Self::decode_loose_bytes(&fs::read(loose)?);
        }
        if self.delta_contains(id) {
            if !visiting.insert(id.to_string()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("delta dependency cycle detected at object {id}")));
            }
            let result = self.read_delta(id, visiting);
            visiting.remove(id.as_str());
            return result;
        }
        if let Some((pack_path, entry)) = self.find_pack_entry(id)? {
            let mut file = fs::File::open(pack_path)?;
            file.seek(SeekFrom::Start(entry.offset))?;
            let stored_size: usize = entry.stored_size.try_into().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pack object too large for this platform"))?;
            let mut stored = vec![0u8; stored_size];
            file.read_exact(&mut stored)?;
            let bytes = match entry.encoding {
                PackEncoding::Raw => stored,
                PackEncoding::Zstd => {
                    #[cfg(feature = "compression")]
                    { decode_zstd_bounded(std::io::Cursor::new(stored), entry.size, "packed object")? }
                    #[cfg(not(feature = "compression"))]
                    { return Err(io::Error::new(io::ErrorKind::Unsupported, "compressed pack entry requires the 'compression' feature")); }
                }
            };
            if bytes.len() as u64 != entry.size {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("packed object {id} decoded to {} bytes; expected {}", bytes.len(), entry.size)));
            }
            return Ok(bytes);
        }
        Err(io::Error::new(io::ErrorKind::NotFound, format!("object {id} not found")))
    }

    fn parse_delta_record(&self, id: &ObjectId) -> io::Result<(DeltaObjectInfo, Vec<u8>)> {
        let path = self.delta_path(id);
        let encoded = fs::read(&path)?;
        if encoded.len() < DELTA_HEADER_LEN || !encoded.starts_with(DELTA_MAGIC) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("invalid delta object header for {id}")));
        }
        let base_text = std::str::from_utf8(&encoded[4..68])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let base = ObjectId::parse(base_text)?;
        let depth = encoded[68];
        let logical_size = u64::from_le_bytes(encoded[69..77].try_into().expect("delta header length checked"));
        if depth == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("delta object {id} has invalid depth 0")));
        }
        Ok((DeltaObjectInfo {
            base,
            depth,
            logical_size,
            physical_size: encoded.len() as u64,
        }, encoded[DELTA_HEADER_LEN..].to_vec()))
    }

    fn read_delta(&self, id: &ObjectId, visiting: &mut HashSet<String>) -> io::Result<Vec<u8>> {
        let (info, payload) = self.parse_delta_record(id)?;
        let base = self.read_internal(&info.base, visiting)?;
        if base.len() as u64 != info.logical_size {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!(
                "delta base {} has {} bytes; target {} expects {}", info.base, base.len(), id, info.logical_size
            )));
        }
        #[cfg(feature = "compression")]
        let delta = decode_zstd_bounded(std::io::Cursor::new(payload), info.logical_size, "delta payload")?;
        #[cfg(not(feature = "compression"))]
        let delta: Vec<u8> = {
            let _ = payload;
            return Err(io::Error::new(io::ErrorKind::Unsupported, "delta objects require the 'compression' feature"));
        };
        if delta.len() != base.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!(
                "delta payload for {id} decoded to {} bytes; expected {}", delta.len(), base.len()
            )));
        }
        let bytes = base.iter().zip(delta.iter()).map(|(a, d)| a ^ d).collect::<Vec<_>>();
        if ObjectId::from_bytes(&bytes) != *id {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("delta object {id} failed reconstructed BLAKE3 verification")));
        }
        Ok(bytes)
    }

    pub fn delta_info(&self, id: &ObjectId) -> io::Result<Option<DeltaObjectInfo>> {
        if !self.delta_contains(id) {
            return Ok(None);
        }
        self.parse_delta_record(id).map(|(info, _)| Some(info))
    }

    pub fn delta_base(&self, id: &ObjectId) -> io::Result<Option<ObjectId>> {
        Ok(self.delta_info(id)?.map(|info| info.base))
    }

    pub fn delta_depth(&self, id: &ObjectId) -> io::Result<u8> {
        Ok(self.delta_info(id)?.map(|info| info.depth).unwrap_or(0))
    }

    fn delta_chain_contains(&self, start: &ObjectId, needle: &ObjectId) -> io::Result<bool> {
        let mut current = start.clone();
        let mut seen = HashSet::new();
        loop {
            if &current == needle { return Ok(true); }
            if !seen.insert(current.to_string()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("existing delta dependency cycle detected at object {current}")));
            }
            let Some(info) = self.delta_info(&current)? else { return Ok(false); };
            current = info.base;
        }
    }

    pub fn optimize_object_as_delta(
        &self,
        target: &ObjectId,
        base: &ObjectId,
        zstd_level: i32,
        min_savings_pct: u8,
        max_depth: u8,
    ) -> io::Result<DeltaInstallResult> {
        if target == base {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes: 0, delta_physical_bytes: 0, savings_bytes: 0, savings_pct: 0.0, depth: 0, reason: Some("target and base are identical".into()) });
        }
        if min_savings_pct > 100 || max_depth == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid delta policy"));
        }
        let target_path = self.object_path(target);
        if !target_path.is_file() {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes: 0, delta_physical_bytes: 0, savings_bytes: 0, savings_pct: 0.0, depth: 0, reason: Some("target is not a loose full object".into()) });
        }
        if self.find_pack_entry(target)?.is_some() {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes: fs::metadata(&target_path)?.len(), delta_physical_bytes: 0, savings_bytes: 0, savings_pct: 0.0, depth: 0, reason: Some("target is also present in a pack; compact/prune before delta optimization".into()) });
        }
        if !self.contains(base) {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("delta base object {base} not found")));
        }
        if self.delta_chain_contains(base, target)? {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes: fs::metadata(&target_path)?.len(), delta_physical_bytes: 0, savings_bytes: 0, savings_pct: 0.0, depth: 0, reason: Some("delta dependency would create a cycle".into()) });
        }
        let base_depth = self.delta_depth(base)?;
        let depth = base_depth.saturating_add(1);
        if depth > max_depth {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes: fs::metadata(&target_path)?.len(), delta_physical_bytes: 0, savings_bytes: 0, savings_pct: 0.0, depth, reason: Some(format!("delta depth {depth} exceeds configured maximum {max_depth}")) });
        }
        let target_bytes = self.read(target)?;
        let base_bytes = self.read(base)?;
        if target_bytes.len() != base_bytes.len() {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes: fs::metadata(&target_path)?.len(), delta_physical_bytes: 0, savings_bytes: 0, savings_pct: 0.0, depth, reason: Some("base and target logical sizes differ".into()) });
        }
        #[cfg(feature = "compression")]
        let compressed_delta = {
            let delta = base_bytes.iter().zip(target_bytes.iter()).map(|(a, b)| a ^ b).collect::<Vec<_>>();
            zstd::stream::encode_all(std::io::Cursor::new(delta), zstd_level)?
        };
        #[cfg(not(feature = "compression"))]
        let compressed_delta: Vec<u8> = {
            let _ = zstd_level;
            return Err(io::Error::new(io::ErrorKind::Unsupported, "persistent delta objects require the 'compression' feature"));
        };
        let mut record = Vec::with_capacity(DELTA_HEADER_LEN + compressed_delta.len());
        record.extend_from_slice(DELTA_MAGIC);
        record.extend_from_slice(base.as_str().as_bytes());
        record.push(depth);
        record.extend_from_slice(&(target_bytes.len() as u64).to_le_bytes());
        record.extend_from_slice(&compressed_delta);

        let full_physical_bytes = fs::metadata(&target_path)?.len();
        let delta_physical_bytes = record.len() as u64;
        let savings_bytes = full_physical_bytes.saturating_sub(delta_physical_bytes);
        let savings_pct = if full_physical_bytes == 0 { 0.0 } else { savings_bytes as f64 / full_physical_bytes as f64 * 100.0 };
        if delta_physical_bytes >= full_physical_bytes || savings_pct + f64::EPSILON < min_savings_pct as f64 {
            return Ok(DeltaInstallResult { stored: false, full_physical_bytes, delta_physical_bytes, savings_bytes, savings_pct, depth, reason: Some(format!("savings {:.2}% below policy threshold {}%", savings_pct, min_savings_pct)) });
        }

        let delta_path = self.delta_path(target);
        if let Some(parent) = delta_path.parent() { fs::create_dir_all(parent)?; }
        Self::atomic_write(&delta_path, &record)?;
        let original_encoded = fs::read(&target_path)?;
        fs::remove_file(&target_path)?;
        match self.verify(target) {
            Ok(true) => Ok(DeltaInstallResult { stored: true, full_physical_bytes, delta_physical_bytes, savings_bytes, savings_pct, depth, reason: None }),
            Ok(false) | Err(_) => {
                let _ = fs::remove_file(&delta_path);
                Self::atomic_write(&target_path, &original_encoded)?;
                Err(io::Error::new(io::ErrorKind::InvalidData, format!("delta representation for {target} failed verification; original object restored")))
            }
        }
    }

    fn list_loose_objects(&self) -> io::Result<Vec<(ObjectId, u64)>> {
        let objects = self.root.join("objects");
        if !objects.exists() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for fanout in fs::read_dir(objects)? {
            let fanout = fanout?;
            if !fanout.path().is_dir() {
                continue;
            }
            let prefix = fanout.file_name().to_string_lossy().to_string();
            if prefix.len() != 2 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            for entry in fs::read_dir(fanout.path())? {
                let entry = entry?;
                if !entry.path().is_file() {
                    continue;
                }
                let suffix = entry.file_name().to_string_lossy().to_string();
                let full = format!("{prefix}{suffix}");
                if let Ok(id) = ObjectId::parse(&full) {
                    result.push((id, entry.metadata()?.len()));
                }
            }
        }
        result.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Ok(result)
    }

    fn load_pack_indexes(&self) -> io::Result<Vec<(PathBuf, PackIndex)>> {
        let packs = self.root.join("packs");
        if !packs.exists() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        let mut paths = fs::read_dir(&packs)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("json")))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let bytes = fs::read(&path)?;
            let mut index: PackIndex = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if index.version != 1 && index.version != PACK_VERSION {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsupported pack index version {}", index.version)));
            }
            let pack_name = Path::new(&index.pack_file);
            let mut components = pack_name.components();
            let is_single_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none();
            if !is_single_name || pack_name.extension().and_then(|value| value.to_str()) != Some("mvpack") {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "pack index contains an invalid pack_file path"));
            }
            for entry in &mut index.objects {
                ObjectId::parse(&entry.id)?;
                if entry.stored_size == 0 { entry.stored_size = entry.size; }
                if index.version == 1 { entry.encoding = PackEncoding::Raw; }
                if entry.encoding == PackEncoding::Raw && entry.stored_size != entry.size {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!(
                        "raw pack object {} stored size {} does not match logical size {}",
                        entry.id, entry.stored_size, entry.size
                    )));
                }
                entry.offset.checked_add(entry.stored_size).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("pack object {} byte range overflows", entry.id))
                })?;
            }
            result.push((path, index));
        }
        Ok(result)
    }

    fn find_pack_entry(&self, id: &ObjectId) -> io::Result<Option<(PathBuf, PackEntry)>> {
        for (index_path, index) in self.load_pack_indexes()? {
            if let Some(entry) = index.objects.iter().find(|entry| entry.id == id.as_str()) {
                let pack_path = index_path.parent().unwrap_or_else(|| Path::new(".")).join(&index.pack_file);
                if !pack_path.is_file() {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("pack file missing: {}", pack_path.display())));
                }
                let pack_len = fs::metadata(&pack_path)?.len();
                let end = entry.offset.checked_add(entry.stored_size).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("pack object {} byte range overflows", entry.id))
                })?;
                if end > pack_len {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!(
                        "pack object {} extends beyond {}",
                        entry.id,
                        pack_path.display()
                    )));
                }
                return Ok(Some((pack_path, entry.clone())));
            }
        }
        Ok(None)
    }

    fn list_delta_objects(&self) -> io::Result<Vec<(ObjectId, u64)>> {
        let deltas = self.root.join("deltas");
        if !deltas.exists() { return Ok(Vec::new()); }
        let mut result = Vec::new();
        for fanout in fs::read_dir(deltas)? {
            let fanout = fanout?;
            if !fanout.path().is_dir() { continue; }
            let prefix = fanout.file_name().to_string_lossy().to_string();
            if prefix.len() != 2 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) { continue; }
            for entry in fs::read_dir(fanout.path())? {
                let entry = entry?;
                let path = entry.path();
                if !path.is_file() || path.extension().and_then(|v| v.to_str()) != Some("mvdelta") { continue; }
                let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or_default();
                let full = format!("{prefix}{stem}");
                if let Ok(id) = ObjectId::parse(&full) {
                    if let Some(info) = self.delta_info(&id)? { result.push((id, info.logical_size)); }
                }
            }
        }
        result.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Ok(result)
    }

    pub fn list_objects(&self) -> io::Result<Vec<(ObjectId, u64)>> {
        let loose = self.list_loose_objects()?;
        let mut result = Vec::with_capacity(loose.len());
        for (id, _) in loose { let logical_size = self.read(&id)?.len() as u64; result.push((id, logical_size)); }
        let mut seen = result.iter().map(|(id, _)| id.to_string()).collect::<HashSet<_>>();
        for (id, logical_size) in self.list_delta_objects()? {
            if seen.insert(id.to_string()) { result.push((id, logical_size)); }
        }
        for (_, index) in self.load_pack_indexes()? {
            for entry in index.objects {
                if seen.insert(entry.id.clone()) {
                    result.push((ObjectId::parse(&entry.id)?, entry.size));
                }
            }
        }
        result.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Ok(result)
    }

    pub fn physical_storage_bytes(&self) -> io::Result<u64> {
        Ok(self.physical_storage_breakdown()?.total())
    }

    pub fn is_loose(&self, id: &ObjectId) -> bool {
        self.loose_contains(id)
    }

    pub fn is_unpacked(&self, id: &ObjectId) -> io::Result<bool> {
        Ok(self.loose_contains(id) || self.delta_contains(id))
    }

    pub fn remove_unpacked_object(&self, id: &ObjectId) -> io::Result<bool> {
        if self.find_pack_entry(id)?.is_some() {
            return Ok(false);
        }
        let mut removed = false;
        let loose = self.object_path(id);
        if loose.is_file() { fs::remove_file(loose)?; removed = true; }
        let delta = self.delta_path(id);
        if delta.is_file() { fs::remove_file(delta)?; removed = true; }
        Ok(removed)
    }

    pub fn verify(&self, id: &ObjectId) -> io::Result<bool> {
        let bytes = self.read(id)?;
        Ok(ObjectId::from_bytes(&bytes) == *id)
    }

    pub fn migrate_loose_compression(&mut self, mode: CompressionMode, zstd_level: i32) -> io::Result<CompressionMigrationReport> {
        let objects = self.list_loose_objects()?;
        let mut report = CompressionMigrationReport::default();
        let previous_mode = self.metadata.loose_compression;
        let previous_level = self.metadata.zstd_level;

        // Decode using the existing representation, then encode with requested policy.
        let decoded = objects
            .iter()
            .map(|(id, physical)| Ok((id.clone(), *physical, self.read(id)?)))
            .collect::<io::Result<Vec<_>>>()?;

        self.set_compression(mode, zstd_level)?;
        for (id, before, bytes) in decoded {
            if ObjectId::from_bytes(&bytes) != id {
                self.set_compression(previous_mode, previous_level)?;
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("object {id} failed verification before migration")));
            }
            let encoded = self.encode_loose(&bytes)?;
            Self::atomic_write(&self.object_path(&id), &encoded)?;
            if !self.verify(&id)? {
                self.set_compression(previous_mode, previous_level)?;
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("object {id} failed verification after migration")));
            }
            report.objects_rewritten += 1;
            report.logical_bytes = report.logical_bytes.saturating_add(bytes.len() as u64);
            report.before_bytes = report.before_bytes.saturating_add(before);
            report.after_bytes = report.after_bytes.saturating_add(encoded.len() as u64);
        }
        Ok(report)
    }

    fn encode_pack_payload(&self, bytes: &[u8]) -> io::Result<(PackEncoding, Vec<u8>)> {
        #[cfg(feature = "compression")]
        {
            let compressed = zstd::stream::encode_all(std::io::Cursor::new(bytes), self.metadata.zstd_level)?;
            if compressed.len() < bytes.len() { return Ok((PackEncoding::Zstd, compressed)); }
        }
        Ok((PackEncoding::Raw, bytes.to_vec()))
    }

    pub fn physical_storage_breakdown(&self) -> io::Result<PhysicalStorageBreakdown> {
        let mut r = PhysicalStorageBreakdown::default();
        for (id, _) in self.list_loose_objects()? {
            let path = self.object_path(&id);
            let bytes = fs::read(&path)?;
            if bytes.starts_with(COMPRESSED_MAGIC) { r.loose_compressed_bytes = r.loose_compressed_bytes.saturating_add(bytes.len() as u64); }
            else { r.loose_raw_bytes = r.loose_raw_bytes.saturating_add(bytes.len() as u64); }
        }
        let deltas = self.root.join("deltas");
        if deltas.exists() {
            for fanout in fs::read_dir(deltas)? {
                let fanout=fanout?; if !fanout.path().is_dir(){continue;}
                for e in fs::read_dir(fanout.path())? { let e=e?; if e.path().is_file(){ r.delta_bytes=r.delta_bytes.saturating_add(e.metadata()?.len()); } }
            }
        }
        let packs=self.root.join("packs");
        if packs.exists(){
            for e in fs::read_dir(packs)? { let e=e?; let p=e.path(); if !p.is_file(){continue;} let n=e.metadata()?.len();
                match p.extension().and_then(|v|v.to_str()) { Some("mvpack")=>r.pack_data_bytes=r.pack_data_bytes.saturating_add(n), Some("json")=>r.pack_index_bytes=r.pack_index_bytes.saturating_add(n), _=>{} }
            }
        }
        let manifests=self.root.join("manifests");
        if manifests.exists(){ for e in fs::read_dir(manifests)? { let e=e?; if e.path().is_file(){r.manifest_bytes=r.manifest_bytes.saturating_add(e.metadata()?.len());} } }
        for name in [REPOSITORY_FILE, "config.json"] { let p=self.root.join(name); if p.is_file(){r.metadata_bytes=r.metadata_bytes.saturating_add(fs::metadata(p)?.len());} }
        Ok(r)
    }

    pub fn representation_sizes(&self, id: &ObjectId) -> io::Result<Vec<u64>> {
        let mut v=Vec::new();
        let p=self.object_path(id); if p.is_file(){v.push(fs::metadata(p)?.len());}
        let d=self.delta_path(id); if d.is_file(){v.push(fs::metadata(d)?.len());}
        for (_, idx) in self.load_pack_indexes()? { for e in idx.objects { if e.id==id.as_str(){v.push(e.stored_size);} } }
        Ok(v)
    }

    /// Estimate the physical size of this object when stored as an independent
    /// full pack-v2 entry using the repository's current compression policy.
    pub fn estimated_full_encoded_size(&self, id: &ObjectId) -> io::Result<u64> {
        let bytes = self.read(id)?;
        let (_, payload) = self.encode_pack_payload(&bytes)?;
        Ok(payload.len() as u64)
    }

    /// Return the on-disk size of a persistent delta representation, if one exists.
    pub fn delta_physical_size(&self, id: &ObjectId) -> io::Result<Option<u64>> {
        let path = self.delta_path(id);
        if path.is_file() { Ok(Some(fs::metadata(path)?.len())) } else { Ok(None) }
    }

    /// Estimate the smallest primary representation ModelVault would retain for
    /// this logical object: full compressed/raw pack entry or an existing delta.
    pub fn estimated_primary_physical_size(&self, id: &ObjectId) -> io::Result<u64> {
        let full = self.estimated_full_encoded_size(id)?;
        Ok(self.delta_physical_size(id)?.map_or(full, |delta| delta.min(full)))
    }

    pub fn repack(&self, prune_loose: bool) -> io::Result<RepackReport> {
        let loose = self.list_loose_objects()?;
        if loose.is_empty() {
            return Ok(RepackReport::default());
        }

        let packed_ids = self.load_pack_indexes()?
            .into_iter()
            .flat_map(|(_, idx)| idx.objects.into_iter().map(|e| e.id))
            .collect::<HashSet<_>>();
        let candidates = loose.into_iter().filter(|(id, _)| !packed_ids.contains(id.as_str())).collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(RepackReport::default());
        }

        let mut name_hasher = blake3::Hasher::new();
        for (id, _) in &candidates {
            name_hasher.update(id.as_str().as_bytes());
        }
        let pack_id = name_hasher.finalize().to_hex().to_string();
        let packs_dir = self.root.join("packs");
        fs::create_dir_all(&packs_dir)?;
        let pack_file_name = format!("pack-{pack_id}.mvpack");
        let index_file_name = format!("pack-{pack_id}.idx.json");
        let pack_path = packs_dir.join(&pack_file_name);
        let index_path = packs_dir.join(&index_file_name);
        let (pack_tmp, mut output) = create_temp_file_near(&pack_path)?;

        let mut entries = Vec::with_capacity(candidates.len());
        let mut offset = 0u64;
        {
            for (id, _) in &candidates {
                let bytes = self.read(id)?;
                if ObjectId::from_bytes(&bytes) != *id {
                    let _ = fs::remove_file(&pack_tmp);
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("object {id} failed verification before repack")));
                }
                let (encoding, payload) = self.encode_pack_payload(&bytes)?;
                output.write_all(&payload)?;
                entries.push(PackEntry { id: id.to_string(), offset, size: bytes.len() as u64, stored_size: payload.len() as u64, encoding });
                offset = offset.saturating_add(payload.len() as u64);
            }
            output.sync_all()?;
        }
        drop(output);
        fs::rename(&pack_tmp, &pack_path)?;
        let index = PackIndex { version: PACK_VERSION, pack_file: pack_file_name, objects: entries.clone() };
        let index_bytes = serde_json::to_vec_pretty(&index).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::atomic_write(&index_path, &index_bytes)?;

        // Verify directly against the new pack before deleting any loose object.
        let mut file = fs::File::open(&pack_path)?;
        for entry in &entries {
            file.seek(SeekFrom::Start(entry.offset))?;
            let mut stored = vec![0u8; entry.stored_size as usize];
            file.read_exact(&mut stored)?;
            let bytes = match entry.encoding { PackEncoding::Raw => stored, PackEncoding::Zstd => { #[cfg(feature="compression")] { decode_zstd_bounded(std::io::Cursor::new(stored), entry.size, "packed object")? } #[cfg(not(feature="compression"))] { return Err(io::Error::new(io::ErrorKind::Unsupported,"compressed pack entry requires compression feature")); } } };
            if bytes.len() as u64 != entry.size || ObjectId::from_bytes(&bytes).as_str() != entry.id {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("packed object {} failed verification", entry.id)));
            }
        }

        let mut report = RepackReport {
            objects_packed: entries.len(),
            logical_bytes: entries.iter().map(|e| e.size).sum(),
            pack_bytes: fs::metadata(&pack_path)?.len(),
            loose_removed: 0,
            pack_path: Some(pack_path),
            index_path: Some(index_path),
        };

        if prune_loose {
            for entry in &entries {
                let id = ObjectId::parse(&entry.id)?;
                let path = self.object_path(&id);
                if path.is_file() {
                    fs::remove_file(path)?;
                    report.loose_removed += 1;
                }
            }
        }
        Ok(report)
    }

    pub fn verify_packs(&self) -> io::Result<PackVerifyReport> {
        let mut report = PackVerifyReport::default();
        for (index_path, index) in self.load_pack_indexes()? {
            report.packs_scanned += 1;
            let pack_path = index_path.parent().unwrap_or_else(|| Path::new(".")).join(&index.pack_file);
            if !pack_path.is_file() {
                report.errors.push(format!("missing pack file {}", pack_path.display()));
                continue;
            }
            let pack_len = fs::metadata(&pack_path)?.len();
            let mut file = fs::File::open(&pack_path)?;
            for entry in &index.objects {
                let end = match entry.offset.checked_add(entry.stored_size) {
                    Some(v) => v,
                    None => {
                        report.errors.push(format!("{}: object {} range overflows", index_path.display(), entry.id));
                        continue;
                    }
                };
                if end > pack_len {
                    report.errors.push(format!("{}: object {} extends beyond pack", index_path.display(), entry.id));
                    continue;
                }
                let size: usize = match entry.stored_size.try_into() {
                    Ok(v) => v,
                    Err(_) => {
                        report.errors.push(format!("{}: object {} too large for this platform", index_path.display(), entry.id));
                        continue;
                    }
                };
                file.seek(SeekFrom::Start(entry.offset))?;
                let mut stored = vec![0u8; size];
                file.read_exact(&mut stored)?;
                let bytes = match entry.encoding {
                    PackEncoding::Raw => stored,
                    PackEncoding::Zstd => {
                        #[cfg(feature="compression")] { decode_zstd_bounded(std::io::Cursor::new(stored), entry.size, "packed object")? }
                        #[cfg(not(feature="compression"))] { report.errors.push(format!("{}: object {} requires compression feature", index_path.display(), entry.id)); continue; }
                    }
                };
                if bytes.len() as u64 != entry.size { report.errors.push(format!("{}: object {} decoded size mismatch", index_path.display(), entry.id)); continue; }
                let actual = ObjectId::from_bytes(&bytes);
                if actual.as_str() != entry.id {
                    report.errors.push(format!("{}: object {} hash mismatch", index_path.display(), entry.id));
                    continue;
                }
                report.objects_verified += 1;
                report.logical_bytes = report.logical_bytes.saturating_add(entry.size);
            }
        }
        Ok(report)
    }

    pub fn compact_packs(&self, prune_old: bool, prune_loose: bool) -> io::Result<PackCompactReport> {
        let objects = self.list_objects()?;
        if objects.is_empty() {
            return Ok(PackCompactReport::default());
        }

        let mut name_hasher = blake3::Hasher::new();
        name_hasher.update(b"compact-v1");
        for (id, _) in &objects {
            name_hasher.update(id.as_str().as_bytes());
        }
        let pack_id = name_hasher.finalize().to_hex().to_string();
        let packs_dir = self.root.join("packs");
        fs::create_dir_all(&packs_dir)?;
        let pack_file_name = format!("pack-{pack_id}.mvpack");
        let index_file_name = format!("pack-{pack_id}.idx.json");
        let pack_path = packs_dir.join(&pack_file_name);
        let index_path = packs_dir.join(&index_file_name);
        let (pack_tmp, mut output) = create_temp_file_near(&pack_path)?;

        let mut entries = Vec::with_capacity(objects.len());
        let mut offset = 0u64;
        {
            for (id, _) in &objects {
                let bytes = self.read(id)?;
                if ObjectId::from_bytes(&bytes) != *id {
                    let _ = fs::remove_file(&pack_tmp);
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("object {id} failed verification before compaction")));
                }
                let (encoding, payload) = self.encode_pack_payload(&bytes)?;
                output.write_all(&payload)?;
                entries.push(PackEntry { id: id.to_string(), offset, size: bytes.len() as u64, stored_size: payload.len() as u64, encoding });
                offset = offset.saturating_add(payload.len() as u64);
            }
            output.sync_all()?;
        }
        drop(output);
        if pack_path.is_file() {
            let _ = fs::remove_file(&pack_tmp);
        } else {
            fs::rename(&pack_tmp, &pack_path)?;
        }
        let index = PackIndex { version: PACK_VERSION, pack_file: pack_file_name.clone(), objects: entries.clone() };
        let index_bytes = serde_json::to_vec_pretty(&index).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::atomic_write(&index_path, &index_bytes)?;

        // Verify the new pack before any destructive cleanup.
        let mut file = fs::File::open(&pack_path)?;
        for entry in &entries {
            file.seek(SeekFrom::Start(entry.offset))?;
            let size: usize = entry.stored_size.try_into().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pack object too large for this platform"))?;
            let mut stored = vec![0u8; size];
            file.read_exact(&mut stored)?;
            let bytes = match entry.encoding { PackEncoding::Raw => stored, PackEncoding::Zstd => { #[cfg(feature="compression")] { decode_zstd_bounded(std::io::Cursor::new(stored), entry.size, "packed object")? } #[cfg(not(feature="compression"))] { return Err(io::Error::new(io::ErrorKind::Unsupported,"compressed pack entry requires compression feature")); } } };
            if bytes.len() as u64 != entry.size || ObjectId::from_bytes(&bytes).as_str() != entry.id {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("compacted object {} failed verification", entry.id)));
            }
        }

        let mut report = PackCompactReport {
            objects_packed: entries.len(),
            logical_bytes: entries.iter().map(|e| e.size).sum(),
            pack_bytes: fs::metadata(&pack_path)?.len(),
            old_pack_files_removed: 0,
            loose_removed: 0,
            pack_path: Some(pack_path.clone()),
            index_path: Some(index_path.clone()),
        };

        if prune_old {
            for entry in fs::read_dir(&packs_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path == pack_path || path == index_path || !path.is_file() {
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mvpack") || e.eq_ignore_ascii_case("json")) {
                    fs::remove_file(path)?;
                    report.old_pack_files_removed += 1;
                }
            }
        }

        if prune_loose {
            for (id, _) in &objects {
                let path = self.object_path(id);
                if path.is_file() {
                    fs::remove_file(path)?;
                    report.loose_removed += 1;
                }
                let delta_path = self.delta_path(id);
                if delta_path.is_file() {
                    fs::remove_file(delta_path)?;
                    report.loose_removed += 1;
                }
            }
        }
        Ok(report)
    }
    pub fn optimize_representations(&self, dry_run: bool) -> io::Result<OptimizeReport> {
        let objects = self.list_objects()?;
        let before = self.physical_storage_breakdown()?.total();
        if objects.is_empty() { return Ok(OptimizeReport { dry_run, before_bytes: before, estimated_after_bytes: before, ..Default::default() }); }
        let mut pack_candidates: Vec<(ObjectId, Vec<u8>, PackEncoding, Vec<u8>)> = Vec::new();
        let mut deltas_retained=0usize;
        let mut estimated=0u64;
        for (id, _) in &objects {
            let bytes=self.read(id)?;
            let (encoding,payload)=self.encode_pack_payload(&bytes)?;
            let pack_size=payload.len() as u64;
            let delta_size=if self.delta_path(id).is_file(){Some(fs::metadata(self.delta_path(id))?.len())}else{None};
            if delta_size.is_some_and(|d| d < pack_size) { deltas_retained+=1; estimated=estimated.saturating_add(delta_size.unwrap()); }
            else { estimated=estimated.saturating_add(pack_size); pack_candidates.push((id.clone(),bytes,encoding,payload)); }
        }
        let mut report=OptimizeReport{dry_run,objects_considered:objects.len(),objects_packed:pack_candidates.len(),deltas_retained,before_bytes:before,estimated_after_bytes:estimated,..Default::default()};
        if dry_run { return Ok(report); }
        let packs_dir=self.root.join("packs"); fs::create_dir_all(&packs_dir)?;
        let mut h=blake3::Hasher::new(); h.update(b"optimize-v2"); for (id,_,_,_) in &pack_candidates{h.update(id.as_str().as_bytes());}
        let pid=h.finalize().to_hex().to_string(); let pf=format!("pack-{pid}.mvpack"); let ix=format!("pack-{pid}.idx.json");
        let pp=packs_dir.join(&pf); let ip=packs_dir.join(&ix); let (tmp, mut out)=create_temp_file_near(&pp)?;
        let mut entries=Vec::new(); let mut offset=0u64; { for (id,bytes,encoding,payload) in &pack_candidates { out.write_all(payload)?; entries.push(PackEntry{id:id.to_string(),offset,size:bytes.len() as u64,stored_size:payload.len() as u64,encoding:*encoding}); offset=offset.saturating_add(payload.len() as u64); } out.sync_all()?; }
        drop(out); fs::rename(&tmp,&pp)?; let idx=PackIndex{version:PACK_VERSION,pack_file:pf,objects:entries}; Self::atomic_write(&ip,&serde_json::to_vec_pretty(&idx).map_err(|e|io::Error::new(io::ErrorKind::InvalidData,e))?)?;
        if !self.verify_packs()?.is_ok(){return Err(io::Error::new(io::ErrorKind::InvalidData,"optimized pack verification failed"));}
        for e in fs::read_dir(&packs_dir)? {
            let e = e?;
            let p = e.path();
            if !p.is_file() || p == pp || p == ip {
                continue;
            }

            if p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x == "mvpack" || x == "json")
            {
                fs::remove_file(p)?;
                report.old_pack_files_removed += 1;
            }
        }
        let packed_ids=pack_candidates.iter().map(|(id,_,_,_)|id.to_string()).collect::<HashSet<_>>();
        for (id,_) in &objects { let lp=self.object_path(id); if lp.is_file(){fs::remove_file(lp)?;report.loose_removed+=1;} let dp=self.delta_path(id); if dp.is_file()&&packed_ids.contains(id.as_str()){fs::remove_file(dp)?;report.deltas_removed+=1;} }
        for (id,_) in &objects { if !self.verify(id)? { return Err(io::Error::new(io::ErrorKind::InvalidData,format!("object {id} failed verification after optimization"))); } }
        report.pack_path=Some(pp); report.index_path=Some(ip); report.estimated_after_bytes=self.physical_storage_breakdown()?.total(); Ok(report)
    }

}
