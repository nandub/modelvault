use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use modelvault::{
    artifact::{add_raw_artifact, add_safetensors_artifact, inspect_safetensors, materialize, materialize_selected_safetensors, verify_artifact},
    benchmark::benchmark_pair,
    cas::{CompressionMode, LocalCas},
    config::{ModelVaultConfig, RemoteDefinition},
    delta::{analyze_delta_potential, optimize_delta_storage},
    diff::diff_models,
    diagnostics::{compare_snapshots, pair_chunk_stats, simulate_policy},
    git_integration::{ensure_modelvault_gitignore, git_add, git_add_force, git_root, pointer_path_for_source, source_is_inside_repo},
    import::{default_hf_cache_dir, default_hf_target, download_hf_file, huggingface_provenance, pointer_path_for_target, repository_target_path, resolve_hf_cached_file},
    lineage::{add_lineage_edge, build_lineage_graph, ensure_no_lineage_cycle, LineageGraphNode},
    manifest::{ArtifactManifest, ArtifactProvenance},
    pointer::ArtifactPointer,
    object_store::{open_remote_store, FilesystemObjectStore},
    remote::{sync_manifest_between, SyncOptions},
    repository::{analytics_report, benchmark_snapshot, fsck, gc, storage_efficiency_report, storage_report},
};

#[derive(Debug, Parser)]
#[command(name = "modelvault", version, about = "Tensor-aware content-addressed storage for AI artifacts")]
struct Cli { #[command(subcommand)] command: Command }

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ArtifactFormat { Auto, Raw, Safetensors }

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompressionArg { None, Zstd }

impl From<CompressionArg> for CompressionMode {
    fn from(value: CompressionArg) -> Self {
        match value { CompressionArg::None => CompressionMode::None, CompressionArg::Zstd => CompressionMode::Zstd }
    }
}

#[derive(Debug, Subcommand)]
enum RemoteCommand {
    /// Add a named filesystem/UNC remote.
    Add { name: String, path: PathBuf, #[arg(long)] default: bool },
    /// Add an AWS S3 or S3-compatible remote.
    AddS3 {
        name: String,
        bucket: String,
        #[arg(long)] prefix: Option<String>,
        #[arg(long)] region: Option<String>,
        #[arg(long)] endpoint: Option<String>,
        #[arg(long)] force_path_style: bool,
        #[arg(long)] profile: Option<String>,
        #[arg(long, default_value_t = 4)] max_attempts: u32,
        #[arg(long)] default: bool,
    },
    /// Add a MinIO remote (S3-compatible, path-style enabled).
    AddMinio {
        name: String,
        bucket: String,
        endpoint: String,
        #[arg(long)] prefix: Option<String>,
        #[arg(long, default_value = "us-east-1")] region: String,
        #[arg(long)] profile: Option<String>,
        #[arg(long, default_value_t = 4)] max_attempts: u32,
        #[arg(long)] default: bool,
    },
    /// List configured remotes.
    List,
    /// Remove a configured remote.
    Remove { name: String },
    /// Set the default remote used when push/pull omit a selector.
    Default { name: String },
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect a Safetensors artifact without loading tensor data into owned memory.
    Inspect { path: PathBuf, #[arg(long)] json: bool },
    /// Initialize a local ModelVault repository.
    Init { #[arg(default_value = ".modelvault")] path: PathBuf },
    /// Add an artifact to the CAS and write a reconstruction manifest.
    Add { path: PathBuf, #[arg(long, value_enum, default_value_t = ArtifactFormat::Auto)] format: ArtifactFormat, #[arg(long, default_value_t = 4 * 1024 * 1024)] chunk_size: usize, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Legacy raw-add command. Prefer `modelvault add`.
    AddRaw { path: PathBuf, #[arg(long, default_value_t = 4 * 1024 * 1024)] chunk_size: usize, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Reconstruct an artifact byte-for-byte from its manifest and CAS objects.
    Materialize { manifest: PathBuf, output: PathBuf, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Verify every object referenced by an artifact manifest.
    Verify { manifest: PathBuf, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Compare chunk reuse between two manifests.
    Compare { left: PathBuf, right: PathBuf },
    /// Track a large artifact using a Git-friendly pointer and manifest.
    Track {
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = ArtifactFormat::Auto)]
        format: ArtifactFormat,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        chunk_size: usize,
        /// Where to write the Git-tracked pointer. Relative paths are resolved from the Git root.
        #[arg(long)]
        pointer: Option<PathBuf>,
        #[arg(long)]
        stage: bool,
    },
    /// Import an external artifact into repository-local ModelVault management without first copying the large file.
    Import {
        source: PathBuf,
        /// Logical repository path the artifact will materialize to on checkout.
        #[arg(long)]
        to: PathBuf,
        #[arg(long, value_enum, default_value_t = ArtifactFormat::Auto)]
        format: ArtifactFormat,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        chunk_size: usize,
        #[arg(long)]
        stage: bool,
    },
    /// Import a Hugging Face model file from the local cache or via the official `hf download` CLI.
    ImportHf {
        repo_id: String,
        #[arg(long, default_value = "model.safetensors")]
        filename: String,
        #[arg(long)]
        revision: Option<String>,
        /// Logical repository path; defaults to models/<repo-name>/<filename>.
        #[arg(long)]
        to: Option<PathBuf>,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Fail instead of invoking `hf download` when the file is not already cached.
        #[arg(long)]
        local_only: bool,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        chunk_size: usize,
        #[arg(long)]
        stage: bool,
    },
    /// Show provenance recorded for a manifest or .mvptr file.
    Provenance { artifact: PathBuf, #[arg(long)] json: bool },
    /// Record that an artifact was derived from another ModelVault artifact.
    Derive {
        artifact: PathBuf,
        #[arg(long)]
        parent: PathBuf,
        /// Derivation operation, e.g. fine-tune, quantize, convert, merge, or distill.
        #[arg(long)]
        operation: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        stage: bool,
    },
    /// Show the recorded ancestry graph for a manifest or .mvptr file.
    Lineage {
        artifact: PathBuf,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 16)]
        max_depth: usize,
    },
    /// Materialize the original artifact described by a .mvptr file.
    Checkout { pointer: PathBuf, #[arg(long)] output: Option<PathBuf> },
    /// Write a derived Safetensors file containing only explicitly selected tensors.
    ExtractTensors {
        pointer: PathBuf,
        #[arg(long = "tensor", required = true)]
        tensors: Vec<String>,
        /// Where to write the derived Safetensors file.
        #[arg(long)]
        output: PathBuf,
        /// Also import the derived file at this logical repository path and record its lineage.
        #[arg(long)]
        to: Option<PathBuf>,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        chunk_size: usize,
        #[arg(long)]
        stage: bool,
    },
    /// Show tensor-aware differences between two manifests or .mvptr files.
    Diff { left: PathBuf, right: PathBuf, #[arg(long)] all: bool },
    /// Benchmark fixed, tensor-fixed, FastCDC, and tensor-FastCDC reuse.
    Benchmark { left: PathBuf, right: PathBuf, #[arg(long, default_value_t = 4 * 1024 * 1024)] avg_chunk_size: usize, #[arg(long)] raw: bool, #[arg(long)] json: bool },
    /// Push all objects referenced by a manifest/pointer to a remote.
    Push {
        artifact: PathBuf,
        #[arg(long)]
        remote: Option<PathBuf>,
        #[arg(long)]
        remote_name: Option<String>,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Read and BLAKE3-hash destination object bodies before reuse.
        #[arg(long)]
        deep_verify: bool,
        #[arg(long, default_value = ".modelvault")]
        store: PathBuf,
    },
    /// Pull all objects referenced by a manifest/pointer from a remote.
    Pull {
        artifact: PathBuf,
        #[arg(long)]
        remote: Option<PathBuf>,
        #[arg(long)]
        remote_name: Option<String>,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Read and BLAKE3-hash destination object bodies before reuse.
        #[arg(long)]
        deep_verify: bool,
        #[arg(long, default_value = ".modelvault")]
        store: PathBuf,
    },
    /// Manage named ModelVault remotes.
    Remote { #[command(subcommand)] command: RemoteCommand, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Check manifests and CAS object integrity for an entire repository.
    Fsck { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] deep: bool },
    /// Report logical, physical, reachable, and orphaned storage.
    Storage { #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Find unreachable CAS objects. Deletion requires --prune.
    Gc { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] prune: bool },
    /// Show repository format/version and current physical object policy.
    RepoInfo { #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Rewrite loose objects using a selected physical compression policy.
    Migrate { #[arg(long, value_enum)] compression: CompressionArg, #[arg(long, default_value_t = 3)] level: i32, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Pack loose CAS objects into an immutable pack/index pair.
    Repack { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] prune_loose: bool },
    /// Verify every pack index entry and object hash.
    PackVerify { #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Compact all known CAS objects into a single verified pack.
    PackCompact { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] prune_old: bool, #[arg(long)] prune_loose: bool },
    /// Estimate storage savings from XOR deltas between aligned changed chunks.
    DeltaAnalyze { left: PathBuf, right: PathBuf, #[arg(long, default_value_t = 3)] level: i32, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Replace eligible loose target chunks with bounded XOR+Zstd delta objects.
    DeltaOptimize { left: PathBuf, right: PathBuf, #[arg(long, default_value_t = 3)] level: i32, #[arg(long)] min_savings_pct: Option<u8>, #[arg(long)] max_depth: Option<u8>, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Configure repository-wide automatic delta storage policy.
    DeltaPolicy { #[arg(long)] min_savings_pct: u8, #[arg(long)] max_depth: u8, #[arg(long, default_value = ".modelvault")] store: PathBuf },
    /// Show repository-wide per-artifact storage attribution and efficiency analytics.
    Analytics { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] detailed: bool, #[arg(long)] json: bool },
    /// Emit a repeatable repository benchmark snapshot for release/strategy comparisons.
    BenchmarkRepo { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] json: bool, #[arg(long)] output: Option<PathBuf> },
    /// Compare two saved repository benchmark snapshots.
    BenchmarkCompare { left: PathBuf, right: PathBuf, #[arg(long)] json: bool },
    /// Show chunk-level compression and reuse diagnostics for one or two artifacts.
    ChunkStats { left: PathBuf, right: Option<PathBuf>, #[arg(long, default_value_t = 3)] level: i32, #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] json: bool },
    /// Simulate fixed-chunk and delta-threshold policies without modifying repository storage.
    SimulatePolicy { left: PathBuf, right: PathBuf, #[arg(long, value_delimiter = ',', default_value = "1048576,2097152,4194304")] chunk_sizes: Vec<usize>, #[arg(long, value_delimiter = ',', default_value = "10,20,30,40")] delta_thresholds: Vec<u8>, #[arg(long, default_value_t = 3)] level: i32, #[arg(long)] json: bool },
    /// Normalize physical representations into compressed pack v2 entries while retaining smaller deltas.
    Optimize { #[arg(long, default_value = ".modelvault")] store: PathBuf, #[arg(long)] dry_run: bool },
}

const CLI_STACK_SIZE: usize = 8 * 1024 * 1024;

fn main() -> anyhow::Result<()> {
    // Clap builds the full command graph before dispatch. On Windows, the
    // default process main-thread stack can be too small as the CLI grows.
    // Run parsing and dispatch on a dedicated stack so even --help/-V remain
    // reliable without changing the public command hierarchy.
    let handle = std::thread::Builder::new()
        .name("modelvault-cli".to_string())
        .stack_size(CLI_STACK_SIZE)
        .spawn(run_cli)?;

    match handle.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect { path, json } => inspect_cmd(path, json)?,
        Command::Init { path } => { let store=LocalCas::open(&path)?; std::fs::create_dir_all(store.root().join("manifests"))?; println!("Initialized ModelVault store at {}",store.root().display()); }
        Command::Add { path, format, chunk_size, store } => { anyhow::ensure!(chunk_size>0,"--chunk-size must be greater than zero"); let cas=LocalCas::open(store)?; let result=add_by_format(&path,&cas,format,chunk_size)?; print_add_result(&path,&result); }
        Command::AddRaw { path, chunk_size, store } => { anyhow::ensure!(chunk_size>0,"--chunk-size must be greater than zero"); let cas=LocalCas::open(store)?; let result=add_raw_artifact(&path,&cas,chunk_size)?; print_add_result(&path,&result); }
        Command::Materialize { manifest, output, store } => { let cas=LocalCas::open(store)?; let manifest=ArtifactManifest::load(&manifest)?; materialize(&manifest,&cas,&output)?; println!("Materialized: {}\nBLAKE3:      {}\nVerified:    byte-for-byte hash match",output.display(),manifest.artifact_id); }
        Command::Verify { manifest, store } => { let cas=LocalCas::open(store)?; let manifest=ArtifactManifest::load(&manifest)?; verify_artifact(&manifest,&cas)?; println!("Artifact: {}\nObjects:  {}\nStatus:   OK",manifest.source_name,manifest.chunks.len()); }
        Command::Compare { left, right } => compare_manifests(&ArtifactManifest::load(left)?,&ArtifactManifest::load(right)?),
        Command::Track { path, format, chunk_size, pointer, stage } => track_cmd(&path, format, chunk_size, pointer.as_deref(), stage)?,
        Command::Import { source, to, format, chunk_size, stage } => import_cmd(&source, &to, format, chunk_size, stage, None)?,
        Command::ImportHf { repo_id, filename, revision, to, cache_dir, local_only, chunk_size, stage } => import_hf_cmd(ImportHfOptions {
            repo_id: &repo_id,
            filename: &filename,
            revision: revision.as_deref(),
            requested_target: to.as_deref(),
            requested_cache_dir: cache_dir.as_deref(),
            local_only,
            chunk_size,
            stage,
        })?,
        Command::Provenance { artifact, json } => provenance_cmd(&artifact, json)?,
        Command::Derive { artifact, parent, operation, note, stage } => derive_cmd(&artifact, &parent, &operation, note.as_deref(), stage)?,
        Command::Lineage { artifact, json, max_depth } => lineage_cmd(&artifact, json, max_depth)?,
        Command::Checkout { pointer, output } => checkout_cmd(&pointer,output.as_deref())?,
        Command::ExtractTensors { pointer, tensors, output, to, chunk_size, stage } => extract_tensors_cmd(&pointer, &tensors, &output, to.as_deref(), chunk_size, stage)?,
        Command::Diff { left, right, all } => diff_cmd(&left,&right,all)?,
        Command::Benchmark { left, right, avg_chunk_size, raw, json } => benchmark_cmd(&left,&right,avg_chunk_size,!raw,json)?,
        Command::Push { artifact, remote, remote_name, jobs, deep_verify, store } => sync_cmd(&artifact, &store, remote.as_deref(), remote_name.as_deref(), jobs, deep_verify, true)?,
        Command::Pull { artifact, remote, remote_name, jobs, deep_verify, store } => sync_cmd(&artifact, &store, remote.as_deref(), remote_name.as_deref(), jobs, deep_verify, false)?,
        Command::Remote { command, store } => remote_cmd(command, &store)?,
        Command::Fsck { store, deep } => fsck_cmd(&store, deep)?,
        Command::Storage { store } => storage_cmd(&store)?,
        Command::Gc { store, prune } => gc_cmd(&store, prune)?,
        Command::RepoInfo { store } => repo_info_cmd(&store)?,
        Command::Migrate { compression, level, store } => migrate_cmd(&store, compression.into(), level)?,
        Command::Repack { store, prune_loose } => repack_cmd(&store, prune_loose)?,
        Command::PackVerify { store } => pack_verify_cmd(&store)?,
        Command::PackCompact { store, prune_old, prune_loose } => pack_compact_cmd(&store, prune_old, prune_loose)?,
        Command::DeltaAnalyze { left, right, level, store } => delta_analyze_cmd(&left, &right, &store, level)?,
        Command::DeltaOptimize { left, right, level, min_savings_pct, max_depth, store } => delta_optimize_cmd(&left, &right, &store, level, min_savings_pct, max_depth)?,
        Command::DeltaPolicy { min_savings_pct, max_depth, store } => delta_policy_cmd(&store, min_savings_pct, max_depth)?,
        Command::Analytics { store, detailed, json } => analytics_cmd(&store, detailed, json)?,
        Command::BenchmarkRepo { store, json, output } => benchmark_repo_cmd(&store, json, output.as_deref())?,
        Command::BenchmarkCompare { left, right, json } => benchmark_compare_cmd(&left, &right, json)?,
        Command::ChunkStats { left, right, level, store, json } => chunk_stats_cmd(&left, right.as_deref(), &store, level, json)?,
        Command::SimulatePolicy { left, right, chunk_sizes, delta_thresholds, level, json } => simulate_policy_cmd(&left, &right, &chunk_sizes, &delta_thresholds, level, json)?,
        Command::Optimize { store, dry_run } => optimize_cmd(&store, dry_run)?,
    }
    Ok(())
}

fn add_by_format(path:&Path,cas:&LocalCas,format:ArtifactFormat,chunk_size:usize)->anyhow::Result<modelvault::artifact::AddArtifactResult>{
    let chosen=match format { ArtifactFormat::Auto=>if path.extension().and_then(|e|e.to_str()).is_some_and(|e|e.eq_ignore_ascii_case("safetensors")){ArtifactFormat::Safetensors}else{ArtifactFormat::Raw}, other=>other };
    match chosen { ArtifactFormat::Safetensors=>add_safetensors_artifact(path,cas,chunk_size), ArtifactFormat::Raw|ArtifactFormat::Auto=>add_raw_artifact(path,cas,chunk_size) }
}

fn inspect_cmd(path:PathBuf,json:bool)->anyhow::Result<()> { let inspection=inspect_safetensors(&path)?; if json { println!("{}",serde_json::to_string_pretty(&inspection)?); } else { println!("Format:       {}\nFile size:    {} bytes\nTensor count: {}\n",inspection.format,inspection.file_size,inspection.tensor_count); println!("{:<54} {:<10} {:<22} {:>12}","Tensor","DType","Shape","Bytes"); println!("{}","-".repeat(104)); for t in inspection.tensors { println!("{:<54} {:<10} {:<22} {:>12}",t.name,t.dtype,format!("{:?}",t.shape),t.byte_len); } } Ok(()) }

fn track_cmd(
    path: &Path,
    format: ArtifactFormat,
    chunk_size: usize,
    requested_pointer: Option<&Path>,
    stage: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(path.exists(), "artifact does not exist: {}", path.display());
    anyhow::ensure!(chunk_size > 0, "--chunk-size must be greater than zero");

    let root = git_root()?;
    anyhow::ensure!(
        source_is_inside_repo(&root, path),
        "track expects an artifact inside the Git repository; use `modelvault import <source> --to <repo-path>` for external files"
    );
    let store_path = root.join(".modelvault");
    let cas = LocalCas::open(&store_path)?;
    let result = add_by_format(path, &cas, format, chunk_size)?;

    let pointer = ArtifactPointer::from_manifest(&result.manifest);
    let pointer_path = pointer_path_for_source(
        &root,
        path,
        &result.manifest.artifact_id,
        requested_pointer,
    )?;

    if stage {
        let canonical_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let pointer_parent = pointer_path.parent().unwrap_or(&root);
        std::fs::create_dir_all(pointer_parent)?;
        let canonical_parent = std::fs::canonicalize(pointer_parent)
            .unwrap_or_else(|_| pointer_parent.to_path_buf());
        anyhow::ensure!(
            canonical_parent.starts_with(&canonical_root),
            "--stage requires the pointer to be inside the Git repository; use --pointer <repo-relative-path>"
        );
    } else if let Some(parent) = pointer_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    pointer.save(&pointer_path)?;
    ensure_modelvault_gitignore(&root, path)?;

    println!(
        "Tracked artifact: {}\nPointer:          {}\nManifest:         {}\nArtifact ID:      {}",
        path.display(),
        pointer_path.display(),
        result.manifest_path.display(),
        result.manifest.artifact_id
    );

    if stage {
        git_add(&[pointer_path.clone(), root.join(".gitignore")])?;
        git_add_force(std::slice::from_ref(&result.manifest_path))?;
        println!("Git:              pointer, manifest, and .gitignore staged");
    } else {
        println!("Git:              not staged (use --stage or git add manually)");
    }
    Ok(())
}


fn import_cmd(
    source: &Path,
    requested_target: &Path,
    format: ArtifactFormat,
    chunk_size: usize,
    stage: bool,
    provenance: Option<ArtifactProvenance>,
) -> anyhow::Result<()> {
    anyhow::ensure!(source.exists(), "artifact does not exist: {}", source.display());
    anyhow::ensure!(chunk_size > 0, "--chunk-size must be greater than zero");

    let root = git_root()?;
    let target = repository_target_path(&root, requested_target)?;
    anyhow::ensure!(
        !target.exists(),
        "import target already exists: {}; use `modelvault track` for an artifact already present in the repository",
        target.display()
    );

    let pointer_path = pointer_path_for_target(&target);
    let cas = LocalCas::open(root.join(".modelvault"))?;
    let mut result = add_by_format(source, &cas, format, chunk_size)?;
    if provenance.is_some() {
        result.manifest.provenance = provenance;
        result.manifest_path = result.manifest.save(cas.root())?;
    }
    let pointer = ArtifactPointer::from_manifest(&result.manifest);
    if let Some(parent) = pointer_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    pointer.save(&pointer_path)?;
    ensure_modelvault_gitignore(&root, &target)?;

    println!(
        "Imported source:  {}\nLogical path:     {}\nPointer:          {}\nManifest:         {}\nArtifact ID:      {}",
        source.display(),
        target.display(),
        pointer_path.display(),
        result.manifest_path.display(),
        result.manifest.artifact_id
    );

    if stage {
        git_add(&[pointer_path, root.join(".gitignore")])?;
        git_add_force(&[result.manifest_path])?;
        println!("Git:              pointer, manifest, and .gitignore staged");
    } else {
        println!("Git:              not staged (use --stage or git add manually)");
    }
    Ok(())
}

struct ImportHfOptions<'a> {
    repo_id: &'a str,
    filename: &'a str,
    revision: Option<&'a str>,
    requested_target: Option<&'a Path>,
    requested_cache_dir: Option<&'a Path>,
    local_only: bool,
    chunk_size: usize,
    stage: bool,
}

fn import_hf_cmd(options: ImportHfOptions<'_>) -> anyhow::Result<()> {
    let ImportHfOptions {
        repo_id,
        filename,
        revision,
        requested_target,
        requested_cache_dir,
        local_only,
        chunk_size,
        stage,
    } = options;

    anyhow::ensure!(!repo_id.trim().is_empty(), "Hugging Face repo ID cannot be empty");
    anyhow::ensure!(!filename.trim().is_empty(), "--filename cannot be empty");

    let cache_dir = match requested_cache_dir {
        Some(path) => path.to_path_buf(),
        None => default_hf_cache_dir()?,
    };

    let cached = resolve_hf_cached_file(&cache_dir, repo_id, revision, filename)?;
    let source = match cached {
        Some(path) => {
            println!("Hugging Face:     using cached file {}", path.display());
            path
        }
        None if local_only => anyhow::bail!(
            "Hugging Face file is not cached: {repo_id}@{} / {filename}",
            revision.unwrap_or("main")
        ),
        None => {
            println!("Hugging Face:     cache miss; invoking `hf download`");
            download_hf_file(repo_id, revision, filename, &cache_dir)?
        }
    };

    let target = requested_target
        .map(PathBuf::from)
        .unwrap_or_else(|| default_hf_target(repo_id, filename));
    let provenance = huggingface_provenance(repo_id, revision, &source, filename);
    import_cmd(
        &source,
        &target,
        ArtifactFormat::Auto,
        chunk_size,
        stage,
        Some(provenance),
    )
}

fn provenance_cmd(path: &Path, json: bool) -> anyhow::Result<()> {
    let manifest = resolve_manifest(path)?;
    let provenance = manifest
        .provenance
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("artifact has no recorded provenance"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(provenance)?);
        return Ok(());
    }

    println!("ModelVault provenance");
    println!("Artifact ID:         {}", manifest.artifact_id);
    println!("Source name:         {}", manifest.source_name);
    println!("Provider:            {}", provenance.provider);
    if let Some(value) = &provenance.namespace { println!("Namespace:           {value}"); }
    if let Some(value) = &provenance.repository { println!("Repository:          {value}"); }
    if let Some(value) = &provenance.model_name { println!("Model:               {value}"); }
    if let Some(value) = &provenance.filename { println!("Filename:            {value}"); }
    if let Some(value) = &provenance.requested_revision { println!("Requested revision:  {value}"); }
    if let Some(value) = &provenance.resolved_revision { println!("Resolved revision:   {value}"); }
    if let Some(value) = &provenance.source_uri { println!("Source URI:          {value}"); }
    Ok(())
}

fn derive_cmd(
    artifact_path: &Path,
    parent_path: &Path,
    operation: &str,
    note: Option<&str>,
    stage: bool,
) -> anyhow::Result<()> {
    let root = git_root()?;
    let store = root.join(".modelvault");

    let (pointer_path, mut child) = if artifact_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mvptr"))
    {
        let pointer = ArtifactPointer::load(artifact_path)?;
        let (_, manifest) = pointer.resolve_manifest(&root)?;
        (Some(artifact_path.to_path_buf()), manifest)
    } else {
        (None, ArtifactManifest::load(artifact_path)?)
    };
    let parent = resolve_manifest(parent_path)?;
    ensure_no_lineage_cycle(&store, &parent, &child.artifact_id)?;

    let added = add_lineage_edge(&mut child, &parent, operation, note)?;
    let manifest_path = child.save(&store)?;

    if let Some(pointer_path) = &pointer_path {
        ArtifactPointer::from_manifest(&child).save(pointer_path)?;
    }

    println!("ModelVault derivation");
    println!("Artifact ID:  {}", child.artifact_id);
    println!("Parent ID:    {}", parent.artifact_id);
    println!("Operation:    {}", operation.trim());
    if let Some(note) = note { println!("Note:         {note}"); }
    println!("Status:       {}", if added { "lineage edge recorded" } else { "lineage edge already present" });

    if stage {
        git_add_force(std::slice::from_ref(&manifest_path))?;
        if let Some(pointer_path) = pointer_path {
            git_add(std::slice::from_ref(&pointer_path))?;
            println!("Git:          manifest and pointer staged");
        } else {
            println!("Git:          manifest staged");
        }
    }
    Ok(())
}

fn lineage_cmd(path: &Path, json: bool, max_depth: usize) -> anyhow::Result<()> {
    anyhow::ensure!(max_depth > 0, "--max-depth must be greater than zero");
    anyhow::ensure!(max_depth <= 256, "--max-depth cannot exceed 256");
    let root = git_root()?;
    let store = root.join(".modelvault");
    let manifest = resolve_manifest(path)?;
    let graph = build_lineage_graph(&store, &manifest, max_depth)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&graph)?);
    } else {
        println!("ModelVault lineage");
        print_lineage_node(&graph, "", true);
    }
    Ok(())
}

fn print_lineage_node(node: &LineageGraphNode, prefix: &str, root: bool) {
    let name = node.source_name.as_deref().unwrap_or("<manifest unavailable>");
    if root {
        println!("{}  {}", node.artifact_id, name);
    }
    if node.truncated {
        println!("{prefix}└── ... maximum depth reached");
        return;
    }
    for (index, edge) in node.parents.iter().enumerate() {
        let last = index + 1 == node.parents.len();
        let branch = if last { "└──" } else { "├──" };
        let parent_name = edge.parent.source_name.as_deref().unwrap_or("<manifest unavailable>");
        let missing = if edge.parent.missing { " [missing]" } else { "" };
        println!(
            "{prefix}{branch} [{}] {}  {}{}",
            edge.operation, edge.parent.artifact_id, parent_name, missing
        );
        if let Some(note) = &edge.note {
            let continuation = if last { "    " } else { "│   " };
            println!("{prefix}{continuation}note: {note}");
        }
        let continuation = if last { "    " } else { "│   " };
        print_lineage_node(&edge.parent, &format!("{prefix}{continuation}"), false);
    }
}

fn checkout_cmd(pointer_path: &Path, output: Option<&Path>) -> anyhow::Result<()> {
    let root = git_root()?;
    let pointer = ArtifactPointer::load(pointer_path)?;
    let (_, manifest) = pointer.resolve_manifest(&root)?;
    let cas = LocalCas::open(root.join(".modelvault"))?;
    let target = output.map(PathBuf::from).unwrap_or_else(|| {
        pointer_path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".mvptr"))
            .map(|n| pointer_path.with_file_name(n))
            .unwrap_or_else(|| PathBuf::from(&pointer.source_name))
    });
    materialize(&manifest, &cas, &target)?;
    println!(
        "Materialized: {}\nArtifact ID:  {}\nVerified:     byte-for-byte hash match",
        target.display(),
        manifest.artifact_id
    );
    Ok(())
}

fn extract_tensors_cmd(
    pointer_path: &Path,
    tensors: &[String],
    output: &Path,
    import_target: Option<&Path>,
    chunk_size: usize,
    stage: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(chunk_size > 0, "--chunk-size must be greater than zero");
    let root = git_root()?;
    let pointer = ArtifactPointer::load(pointer_path)?;
    let (_, manifest) = pointer.resolve_manifest(&root)?;
    let cas = LocalCas::open(root.join(".modelvault"))?;
    let result = materialize_selected_safetensors(&manifest, &cas, tensors, output)?;
    println!(
        "Derived Safetensors: {}\nSource artifact ID:  {}\nDerived artifact ID: {}\nTensors:             {}\nLogical size:        {} bytes\nVerified:            source byte-for-byte hash match",
        output.display(),
        result.source_artifact_id,
        result.derived_artifact_id,
        result.tensor_count,
        result.logical_size,
    );
    if let Some(target) = import_target {
        import_cmd(output, target, ArtifactFormat::Safetensors, chunk_size, stage, None)?;
        let logical_target = repository_target_path(&root, target)?;
        let derived_pointer_path = pointer_path_for_target(&logical_target);
        let derived_pointer = ArtifactPointer::load(&derived_pointer_path)?;
        anyhow::ensure!(
            derived_pointer.artifact_id.eq_ignore_ascii_case(&result.derived_artifact_id),
            "imported derived artifact ID does not match extracted output"
        );
        derive_cmd(&derived_pointer_path, pointer_path, "extract-tensors", None, stage)?;
    }
    Ok(())
}

fn resolve_manifest(path: &Path) -> anyhow::Result<ArtifactManifest> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mvptr"))
    {
        let root = git_root()?;
        let pointer = ArtifactPointer::load(path)?;
        let (_, manifest) = pointer.resolve_manifest(&root)?;
        Ok(manifest)
    } else {
        Ok(ArtifactManifest::load(path)?)
    }
}

fn diff_cmd(left:&Path,right:&Path,all:bool)->anyhow::Result<()> { let l=resolve_manifest(left)?; let r=resolve_manifest(right)?; let d=diff_models(&l,&r); println!("Model diff\nLeft:      {}\nRight:     {}\n\nTensors\n-------\nUnchanged: {}\nChanged:   {}\nAdded:     {}\nRemoved:   {}",l.source_name,r.source_name,d.unchanged,d.changed,d.added,d.removed); println!("\n{:<58} {:<10} {:>12} {:>9}","Tensor","Status","Right bytes","Reuse"); println!("{}","-".repeat(94)); for t in d.tensors.iter().filter(|t|all||t.status!="unchanged") { let pct=if t.right_bytes==0{0.0}else{t.shared_bytes as f64/t.right_bytes as f64*100.0}; println!("{:<58} {:<10} {:>12} {:>8.2}%",t.name,t.status,t.right_bytes,pct); } Ok(()) }

fn benchmark_cmd(left:&Path,right:&Path,avg:usize,safetensors:bool,json:bool)->anyhow::Result<()> { anyhow::ensure!(avg>0,"--avg-chunk-size must be greater than zero"); let rows=benchmark_pair(left,right,avg,safetensors)?; if json { let results:Vec<_>=rows.iter().map(|r|serde_json::json!({"strategy":r.strategy,"left_chunks":r.left_chunks,"right_chunks":r.right_chunks,"shared_bytes":r.shared_bytes,"right_size":r.right_size,"reuse_pct":r.reuse_pct,"elapsed_ms":r.elapsed.as_secs_f64()*1000.0})).collect(); println!("{}",serde_json::to_string_pretty(&serde_json::json!({"format_version":1,"left":left,"right":right,"avg_chunk_size":avg,"safetensors":safetensors,"results":results}))?); return Ok(()); } println!("Chunking benchmark\nLeft:  {}\nRight: {}\nAverage target: {} bytes\n",left.display(),right.display(),avg); println!("{:<18} {:>12} {:>12} {:>14} {:>10} {:>12}","Strategy","Left chunks","Right chunks","Shared bytes","Reuse","Time ms"); println!("{}","-".repeat(84)); for r in rows { println!("{:<18} {:>12} {:>12} {:>14} {:>9.2}% {:>12.2}",r.strategy,r.left_chunks,r.right_chunks,r.shared_bytes,r.reuse_pct,r.elapsed.as_secs_f64()*1000.0); } Ok(()) }

fn print_add_result(path:&Path,result:&modelvault::artifact::AddArtifactResult){ let logical=result.manifest.logical_size; let reuse_pct=if logical==0{0.0}else{result.reused_bytes as f64/logical as f64*100.0}; println!("Artifact:      {}\nFormat:        {}\nArtifact ID:   {}\nLogical size:  {} bytes\nChunks:        {}\nTensors:       {}\nNew bytes:     {}\nReused bytes:  {}\nReuse:         {:.2}%\nManifest:      {}",path.display(),result.manifest.format,result.manifest.artifact_id,logical,result.manifest.chunks.len(),result.manifest.tensors.len(),result.new_bytes,result.reused_bytes,reuse_pct,result.manifest_path.display()); }

fn compare_manifests(left:&ArtifactManifest,right:&ArtifactManifest){ use std::collections::HashMap; let left_objects:HashMap<&str,u64>=left.chunks.iter().map(|c|(c.object.as_str(),c.size)).collect(); let mut shared_bytes=0; let mut shared_chunks=0; for c in &right.chunks { if left_objects.contains_key(c.object.as_str()){shared_bytes+=c.size;shared_chunks+=1;} } let right_unique=right.logical_size.saturating_sub(shared_bytes); let reuse_pct=if right.logical_size==0{0.0}else{shared_bytes as f64/right.logical_size as f64*100.0}; println!("Left:             {}\nRight:            {}\nShared chunks:    {} / {}\nShared bytes:     {}\nRight-only bytes: {}\nRight reuse:      {:.2}%",left.source_name,right.source_name,shared_chunks,right.chunks.len(),shared_bytes,right_unique,reuse_pct); }


fn resolve_remote_definition(
    store: &Path,
    direct: Option<&Path>,
    name: Option<&str>,
) -> anyhow::Result<(String, RemoteDefinition)> {
    anyhow::ensure!(direct.is_none() || name.is_none(), "use either --remote or --remote-name, not both");
    if let Some(path) = direct {
        return Ok((path.display().to_string(), RemoteDefinition::filesystem(path.to_path_buf())));
    }
    let config = ModelVaultConfig::load(store)?;
    config.resolve(name)
}

fn sync_cmd(
    artifact: &Path,
    store: &Path,
    direct_remote: Option<&Path>,
    remote_name: Option<&str>,
    jobs: usize,
    deep_verify: bool,
    push: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(jobs > 0, "--jobs must be greater than zero");
    let manifest = resolve_manifest(artifact)?;
    let local = FilesystemObjectStore::open(store.to_path_buf())?;
    let (remote_label, definition) = resolve_remote_definition(store, direct_remote, remote_name)?;
    let remote = open_remote_store(&definition)?;
    let options = SyncOptions { jobs, deep_verify };
    let result = if push {
        sync_manifest_between(&manifest, &local, remote.as_ref(), &options)?
    } else {
        sync_manifest_between(&manifest, remote.as_ref(), &local, &options)?
    };
    let mib_per_sec = if result.elapsed_ms == 0 {
        0.0
    } else {
        (result.bytes_copied as f64 / 1_048_576.0) / (result.elapsed_ms as f64 / 1000.0)
    };
    println!("Verification:   {}", if deep_verify { "deep content hash" } else { "fast/backend" });
    println!("{}\nArtifact:       {}\nRemote:         {}\nRemote type:    {}\nJobs:           {}\nUnique objects: {}\nCopied objects: {}\nReused objects: {}\nCopied bytes:   {}\nReused bytes:   {}\nElapsed:        {} ms\nTransfer rate:  {:.2} MiB/s\nManifest:       {}",
        if push { "Push complete" } else { "Pull complete" },
        manifest.source_name,
        remote_label,
        definition.kind,
        jobs,
        result.objects_total,
        result.objects_copied,
        result.objects_reused,
        result.bytes_copied,
        result.bytes_reused,
        result.elapsed_ms,
        mib_per_sec,
        result.manifest_path.display());
    Ok(())
}

fn warn_if_insecure_endpoint(endpoint: &str) {
    let value = endpoint.trim().to_ascii_lowercase();
    let Some(authority) = value.strip_prefix("http://").and_then(|rest| rest.split('/').next()) else {
        return;
    };
    let host = if authority.starts_with('[') {
        authority.split(']').next().unwrap_or(authority).trim_start_matches('[')
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    let local = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !local {
        eprintln!("Warning: remote endpoint uses plaintext HTTP; use HTTPS for non-local S3/MinIO endpoints");
    }
}

fn remote_cmd(command: RemoteCommand, store: &Path) -> anyhow::Result<()> {
    let mut config = ModelVaultConfig::load(store)?;
    match command {
        RemoteCommand::Add { name, path, default } => {
            config.add_filesystem_remote(&name, path.clone())?;
            if default { config.set_default(&name)?; }
            let config_path = config.save(store)?;
            println!("Added remote '{}'\nType:    filesystem\nPath:    {}\nDefault: {}\nConfig:  {}",
                name, path.display(), if default { "yes" } else { "no" }, config_path.display());
        }
        RemoteCommand::AddS3 { name, bucket, prefix, region, endpoint, force_path_style, profile, max_attempts, default } => {
            if let Some(value) = endpoint.as_deref() { warn_if_insecure_endpoint(value); }
            let remote = RemoteDefinition::s3(
                bucket.clone(), prefix.clone(), region.clone(), endpoint.clone(),
                force_path_style, profile.clone(), max_attempts,
            )?;
            config.add_s3_remote(&name, remote)?;
            if default { config.set_default(&name)?; }
            let config_path = config.save(store)?;
            println!("Added remote '{}'\nType:       s3\nBucket:     {}\nPrefix:     {}\nRegion:     {}\nEndpoint:   {}\nPath style: {}\nProfile:    {}\nAttempts:   {}\nDefault:    {}\nConfig:     {}",
                name, bucket, prefix.as_deref().unwrap_or("(none)"), region.as_deref().unwrap_or("SDK default"),
                endpoint.as_deref().unwrap_or("AWS default"), if force_path_style { "yes" } else { "no" },
                profile.as_deref().unwrap_or("SDK default"), max_attempts, if default { "yes" } else { "no" }, config_path.display());
        }
        RemoteCommand::AddMinio { name, bucket, endpoint, prefix, region, profile, max_attempts, default } => {
            warn_if_insecure_endpoint(&endpoint);
            let remote = RemoteDefinition::s3(
                bucket.clone(), prefix.clone(), Some(region.clone()), Some(endpoint.clone()),
                true, profile.clone(), max_attempts,
            )?;
            config.add_s3_remote(&name, remote)?;
            if default { config.set_default(&name)?; }
            let config_path = config.save(store)?;
            println!("Added remote '{}'\nType:       s3 (MinIO)\nBucket:     {}\nPrefix:     {}\nRegion:     {}\nEndpoint:   {}\nPath style: yes\nProfile:    {}\nAttempts:   {}\nDefault:    {}\nConfig:     {}",
                name, bucket, prefix.as_deref().unwrap_or("(none)"), region, endpoint,
                profile.as_deref().unwrap_or("SDK default"), max_attempts, if default { "yes" } else { "no" }, config_path.display());
        }
        RemoteCommand::List => {
            println!("ModelVault remotes");
            if config.remotes.is_empty() {
                println!("(none configured)");
            } else {
                for (name, remote) in &config.remotes {
                    let marker = if config.default_remote.as_deref() == Some(name.as_str()) { "*" } else { " " };
                    let location = match remote.kind.as_str() {
                        "filesystem" => remote.path.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "<missing path>".to_string()),
                        "s3" => {
                            let bucket = remote.bucket.as_deref().unwrap_or("<missing bucket>");
                            let prefix = remote.prefix.as_deref().map(|p| format!("/{p}")).unwrap_or_default();
                            match &remote.endpoint {
                                Some(endpoint) => format!("s3://{bucket}{prefix} @ {endpoint}"),
                                None => format!("s3://{bucket}{prefix}"),
                            }
                        }
                        _ => "<unsupported>".to_string(),
                    };
                    println!("{} {:<20} {:<12} {}", marker, name, remote.kind, location);
                }
                println!("\n* default remote");
            }
        }
        RemoteCommand::Remove { name } => {
            config.remove_remote(&name)?;
            config.save(store)?;
            println!("Removed remote '{}'", name);
        }
        RemoteCommand::Default { name } => {
            config.set_default(&name)?;
            config.save(store)?;
            println!("Default remote: {}", name);
        }
    }
    Ok(())
}

fn fsck_cmd(store: &Path, deep: bool) -> anyhow::Result<()> {
    let report = fsck(store, deep)?;
    println!("ModelVault fsck\nStore:              {}\nMode:               {}\nManifests scanned:  {}\nManifests OK:       {}\nReferenced objects: {}\nMissing objects:    {}\nCorrupt objects:    {}\nOrphan objects:     {}",
        store.display(), if deep { "deep" } else { "structural" }, report.manifests_scanned,
        report.manifests_ok, report.referenced_objects, report.missing_objects,
        report.corrupt_objects, report.orphan_objects);
    if !report.manifest_errors.is_empty() {
        println!("\nErrors\n------");
        for error in &report.manifest_errors { println!("- {error}"); }
    }
    anyhow::ensure!(report.is_ok(), "repository integrity check failed");
    Ok(())
}

fn storage_cmd(store: &Path) -> anyhow::Result<()> {
    let r = storage_report(store)?;
    let e = storage_efficiency_report(store)?;
    println!("ModelVault storage\nStore:                         {}\nManifests:                     {}\nLogical bytes:                 {}\nUnique reachable bytes:        {}\nPhysical repository bytes:     {}\nReachable objects:             {}\nOrphan objects:                {}\nTrue orphan physical bytes:    {}\nDuplicate representation bytes:{}\n\nPhysical storage\n----------------\nLoose raw bytes:               {}\nLoose compressed bytes:        {}\nDelta bytes:                   {}\nPack data bytes:               {}\nPack index bytes:              {}\nManifest bytes:                {}\nMetadata/config bytes:         {}\n\nStorage efficiency\n------------------\nDedup savings:                 {} ({:.2}%)\nCompression savings:           {} ({:.2}%)\nDelta savings:                 {} ({:.2}%)\nMetadata overhead:             {}\nNet physical savings:          {} ({:.2}%)",
        store.display(), r.manifests, r.logical_bytes, r.reachable_bytes, r.physical_bytes,
        r.reachable_objects, r.orphan_objects, r.orphan_bytes, r.duplicate_representation_bytes,
        r.loose_raw_bytes, r.loose_compressed_bytes, r.delta_bytes, r.pack_data_bytes,
        r.pack_index_bytes, r.manifest_bytes, r.metadata_bytes,
        e.dedup_savings_bytes, e.dedup_savings_pct,
        e.compression_savings_bytes, e.compression_savings_pct,
        e.delta_savings_bytes, e.delta_savings_pct,
        e.metadata_overhead_bytes,
        format_signed_bytes(e.net_physical_savings_bytes), e.net_physical_savings_pct);
    Ok(())
}

fn format_signed_bytes(value: i128) -> String {
    if value >= 0 { value.to_string() } else { format!("-{}", value.unsigned_abs()) }
}

fn gc_cmd(store: &Path, prune: bool) -> anyhow::Result<()> {
    let r = gc(store, prune)?;
    println!("ModelVault gc\nStore:           {}\nMode:            {}\nOrphan objects:  {}\nOrphan bytes:    {}\nRemoved objects: {}\nRemoved bytes:   {}",
        store.display(), if prune { "prune" } else { "dry-run" },
        r.orphan_objects, r.orphan_bytes, r.removed_objects, r.removed_bytes);
    if !prune && r.orphan_objects > 0 {
        println!("No objects were deleted. Re-run with --prune to remove unreachable objects.");
    }
    Ok(())
}


fn repo_info_cmd(store: &Path) -> anyhow::Result<()> {
    let cas = LocalCas::open(store)?;
    let metadata = cas.metadata();
    println!(
        "ModelVault repository\nStore:               {}\nRepository version:  {}\nObject hash:         {}\nLoose compression:   {}\nZstd level:          {}\nPack format version: {}\nDelta min savings:   {}%\nMax delta depth:     {}",
        store.display(), metadata.version, metadata.object_hash, metadata.loose_compression,
        metadata.zstd_level, metadata.pack_format_version, metadata.delta_min_savings_pct, metadata.max_delta_depth
    );
    Ok(())
}

fn migrate_cmd(store: &Path, compression: CompressionMode, level: i32) -> anyhow::Result<()> {
    anyhow::ensure!((-7..=22).contains(&level), "--level must be between -7 and 22");
    let mut cas = LocalCas::open(store)?;
    let report = cas.migrate_loose_compression(compression, level)?;
    let saved = report.before_bytes.saturating_sub(report.after_bytes);
    let pct = if report.before_bytes == 0 { 0.0 } else { saved as f64 / report.before_bytes as f64 * 100.0 };
    println!(
        "ModelVault migration\nStore:             {}\nCompression:       {}\nObjects rewritten: {}\nLogical bytes:     {}\nBefore bytes:      {}\nAfter bytes:       {}\nPhysical savings:  {} ({:.2}%)",
        store.display(), compression, report.objects_rewritten, report.logical_bytes,
        report.before_bytes, report.after_bytes, saved, pct
    );
    Ok(())
}

fn repack_cmd(store: &Path, prune_loose: bool) -> anyhow::Result<()> {
    let cas = LocalCas::open(store)?;
    let report = cas.repack(prune_loose)?;
    println!(
        "ModelVault repack\nStore:          {}\nObjects packed: {}\nLogical bytes:  {}\nPack bytes:     {}\nLoose removed:  {}",
        store.display(), report.objects_packed, report.logical_bytes, report.pack_bytes, report.loose_removed
    );
    if let Some(path) = report.pack_path { println!("Pack:           {}", path.display()); }
    if let Some(path) = report.index_path { println!("Index:          {}", path.display()); }
    if !prune_loose && report.objects_packed > 0 {
        println!("Loose objects were retained. Re-run with --prune-loose after validation to reclaim loose-object space.");
    }
    Ok(())
}


fn pack_verify_cmd(store: &Path) -> anyhow::Result<()> {
    let cas = LocalCas::open(store)?;
    let report = cas.verify_packs()?;
    println!("ModelVault pack verification\nPacks scanned:     {}\nObjects verified:  {}\nLogical bytes:     {}\nErrors:            {}",
        report.packs_scanned, report.objects_verified, report.logical_bytes, report.errors.len());
    if !report.errors.is_empty() {
        println!("\nErrors\n------");
        for error in &report.errors { println!("- {error}"); }
    }
    anyhow::ensure!(report.is_ok(), "pack verification failed");
    Ok(())
}

fn pack_compact_cmd(store: &Path, prune_old: bool, prune_loose: bool) -> anyhow::Result<()> {
    let cas = LocalCas::open(store)?;
    let report = cas.compact_packs(prune_old, prune_loose)?;
    println!("ModelVault pack compaction\nObjects packed:    {}\nLogical bytes:     {}\nPack bytes:        {}\nOld pack files removed: {}\nLoose objects removed:  {}",
        report.objects_packed, report.logical_bytes, report.pack_bytes, report.old_pack_files_removed, report.loose_removed);
    if let Some(path) = report.pack_path { println!("Pack:              {}", path.display()); }
    if let Some(path) = report.index_path { println!("Index:             {}", path.display()); }
    Ok(())
}

fn optimize_cmd(store: &Path, dry_run: bool) -> anyhow::Result<()> {
    let cas=LocalCas::open(store)?; let r=cas.optimize_representations(dry_run)?;
    println!("ModelVault optimize\nMode:                    {}\nObjects considered:      {}\nObjects packed:          {}\nDeltas retained:         {}\nBefore physical bytes:   {}\nAfter/estimated bytes:   {}\nOld pack files removed:  {}\nLoose objects removed:   {}\nDelta objects removed:   {}", if dry_run{"dry-run"}else{"apply"}, r.objects_considered,r.objects_packed,r.deltas_retained,r.before_bytes,r.estimated_after_bytes,r.old_pack_files_removed,r.loose_removed,r.deltas_removed);
    if let Some(p) = r.pack_path {
        println!("Pack:                    {}", p.display());
    }
    if let Some(p) = r.index_path {
        println!("Index:                   {}", p.display());
    }
    Ok(())
}

fn delta_analyze_cmd(left: &Path, right: &Path, store: &Path, level: i32) -> anyhow::Result<()> {
    let left = resolve_manifest(left)?;
    let right = resolve_manifest(right)?;
    let cas = LocalCas::open(store)?;
    let report = analyze_delta_potential(&left, &right, &cas, level)?;
    println!("Delta storage analysis\nLeft:                   {}\nRight:                  {}\nChanged chunks:         {}\nComparable chunks:      {}\nIncomparable bytes:     {}\nFull Zstd bytes:        {}\nXOR-delta Zstd bytes:   {}\nPotential savings:      {} ({:.2}%)",
        left.source_name, right.source_name, report.right_changed_chunks, report.comparable_chunks,
        report.incomparable_bytes, report.full_compressed_bytes, report.delta_compressed_bytes,
        report.potential_savings_bytes, report.potential_savings_pct);
    Ok(())
}

fn delta_optimize_cmd(
    left: &Path,
    right: &Path,
    store: &Path,
    level: i32,
    min_savings_pct: Option<u8>,
    max_depth: Option<u8>,
) -> anyhow::Result<()> {
    anyhow::ensure!((-7..=22).contains(&level), "--level must be between -7 and 22");
    let left = resolve_manifest(left)?;
    let right = resolve_manifest(right)?;
    let cas = LocalCas::open(store)?;
    let min_savings_pct = min_savings_pct.unwrap_or(cas.metadata().delta_min_savings_pct);
    let max_depth = max_depth.unwrap_or(cas.metadata().max_delta_depth);
    anyhow::ensure!(min_savings_pct <= 100, "--min-savings-pct must be <= 100");
    anyhow::ensure!(max_depth > 0, "--max-depth must be greater than zero");
    let report = optimize_delta_storage(&left, &right, &cas, level, min_savings_pct, max_depth)?;
    println!(
        "Delta storage optimization\nLeft:                  {}\nRight:                 {}\nPolicy minimum saving: {}%\nPolicy max depth:      {}\nCandidates:            {}\nStored as delta:       {}\nSkipped:               {}\nFull physical bytes:   {}\nDelta physical bytes:  {}\nPhysical savings:      {} ({:.2}%)\nMax depth observed:    {}",
        left.source_name, right.source_name, min_savings_pct, max_depth, report.candidates,
        report.stored, report.skipped, report.full_physical_bytes, report.delta_physical_bytes,
        report.savings_bytes, report.savings_pct, report.max_depth_observed
    );
    Ok(())
}

fn delta_policy_cmd(store: &Path, min_savings_pct: u8, max_depth: u8) -> anyhow::Result<()> {
    anyhow::ensure!(min_savings_pct <= 100, "--min-savings-pct must be <= 100");
    anyhow::ensure!(max_depth > 0, "--max-depth must be greater than zero");
    let mut cas = LocalCas::open(store)?;
    cas.set_delta_policy(min_savings_pct, max_depth)?;
    println!(
        "ModelVault delta policy\nStore:               {}\nMinimum savings:     {}%\nMaximum delta depth: {}",
        store.display(), min_savings_pct, max_depth
    );
    Ok(())
}

fn analytics_cmd(store: &Path, detailed: bool, json: bool) -> anyhow::Result<()> {
    let report = analytics_report(store)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("ModelVault analytics\nTotal logical bytes:         {}\nUnique reachable bytes:      {}\nPhysical repository bytes:   {}\nDedup savings:               {} ({:.2}%)\nCompression savings:         {} ({:.2}%)\nDelta savings:               {} ({:.2}%)\nNet physical savings:        {} ({:.2}%)\nPhysical/logical ratio:      {:.2}%",
        report.total_logical_bytes, report.unique_logical_object_bytes, report.physical_bytes,
        report.efficiency.dedup_savings_bytes, report.efficiency.dedup_savings_pct,
        report.efficiency.compression_savings_bytes, report.efficiency.compression_savings_pct,
        report.efficiency.delta_savings_bytes, report.efficiency.delta_savings_pct,
        format_signed_bytes(report.efficiency.net_physical_savings_bytes), report.efficiency.net_physical_savings_pct,
        report.physical_ratio);
    println!("\n{:<42} {:>14} {:>14} {:>14} {:>14} {:>9}", "Artifact", "Logical", "Exclusive", "Shared", "Attributed", "Shared");
    println!("{}", "-".repeat(114));
    for artifact in &report.artifacts {
        println!("{:<42} {:>14} {:>14} {:>14} {:>14} {:>8.2}%", artifact.source_name,
            artifact.logical_bytes, artifact.exclusive_bytes, artifact.shared_bytes,
            artifact.attributed_physical_bytes, artifact.shared_pct);
    }
    if detailed {
        println!("\nEfficiency decomposition\n------------------------\nUnique logical bytes:         {}\nBest full-object encoding:    {}\nBest primary representation:  {}\nDuplicate representations:    {}\nMetadata/index overhead:       {}",
            report.efficiency.unique_logical_bytes, report.efficiency.full_encoded_bytes,
            report.efficiency.primary_encoded_bytes, report.efficiency.duplicate_representation_bytes,
            report.efficiency.metadata_overhead_bytes);
    }
    Ok(())
}

fn benchmark_repo_cmd(store: &Path, json: bool, output: Option<&Path>) -> anyhow::Result<()> {
    let snapshot = benchmark_snapshot(store)?;
    let serialized = serde_json::to_string_pretty(&snapshot)?;
    if let Some(path) = output {
        std::fs::write(path, serialized.as_bytes())?;
        println!("Benchmark snapshot written: {}", path.display());
    }
    if json || output.is_none() {
        if json {
            println!("{serialized}");
        } else {
            let e = &snapshot.efficiency;
            println!("ModelVault repository benchmark\nFormat version:       {}\nManifests:            {}\nReachable objects:    {}\nLogical bytes:        {}\nUnique logical bytes: {}\nFull encoded bytes:   {}\nPrimary encoded bytes:{}\nPhysical bytes:       {}\nDedup savings:        {:.2}%\nCompression savings:  {:.2}%\nDelta savings:        {:.2}%\nNet physical savings: {:.2}%",
                snapshot.format_version, snapshot.manifests, snapshot.objects, e.logical_bytes,
                e.unique_logical_bytes, e.full_encoded_bytes, e.primary_encoded_bytes,
                e.actual_physical_bytes, e.dedup_savings_pct, e.compression_savings_pct,
                e.delta_savings_pct, e.net_physical_savings_pct);
        }
    }
    Ok(())
}



fn benchmark_compare_cmd(left: &Path, right: &Path, json: bool) -> anyhow::Result<()> {
    let l: modelvault::repository::RepositoryBenchmarkSnapshot = serde_json::from_slice(&std::fs::read(left)?)?;
    let r: modelvault::repository::RepositoryBenchmarkSnapshot = serde_json::from_slice(&std::fs::read(right)?)?;
    let d = compare_snapshots(&l, &r);
    if json { println!("{}", serde_json::to_string_pretty(&d)?); return Ok(()); }
    println!("ModelVault benchmark comparison\nMetric                     Before          After         Change\n--------------------------------------------------------------------------\nLogical bytes        {:>14} {:>14} {:>+14}\nPhysical bytes       {:>14} {:>14} {:>+14}\nDedup savings          {:>11.2}% {:>11.2}% {:>+10.2} pp\nCompression savings    {:>11.2}% {:>11.2}% {:>+10.2} pp\nDelta savings          {:>11.2}% {:>11.2}% {:>+10.2} pp\nNet savings            {:>11.2}% {:>11.2}% {:>+10.2} pp",
        l.efficiency.logical_bytes, r.efficiency.logical_bytes, d.logical_bytes,
        l.efficiency.actual_physical_bytes, r.efficiency.actual_physical_bytes, d.physical_bytes,
        l.efficiency.dedup_savings_pct, r.efficiency.dedup_savings_pct, d.dedup_savings_pct_points,
        l.efficiency.compression_savings_pct, r.efficiency.compression_savings_pct, d.compression_savings_pct_points,
        l.efficiency.delta_savings_pct, r.efficiency.delta_savings_pct, d.delta_savings_pct_points,
        l.efficiency.net_physical_savings_pct, r.efficiency.net_physical_savings_pct, d.net_savings_pct_points);
    Ok(())
}

fn chunk_stats_cmd(left: &Path, right: Option<&Path>, store: &Path, level: i32, json: bool) -> anyhow::Result<()> {
    let cas = LocalCas::open(store)?; let l = resolve_manifest(left)?;
    if let Some(rp) = right { let r=resolve_manifest(rp)?; let report=pair_chunk_stats(&l,&r,&cas,level)?; if json { println!("{}",serde_json::to_string_pretty(&report)?); return Ok(()); }
        println!("ModelVault pair chunk statistics\nLeft:          {}\nRight:         {}\nShared objects:{}\nShared bytes:  {}\n", report.left.artifact, report.right.artifact, report.shared_objects, report.shared_bytes);
        print_chunk_stats(&report.left); println!(); print_chunk_stats(&report.right);
    } else { let report=modelvault::diagnostics::chunk_stats(&l,&cas,level)?; if json { println!("{}",serde_json::to_string_pretty(&report)?); } else { print_chunk_stats(&report); } }
    Ok(())
}

fn print_chunk_stats(r:&modelvault::diagnostics::ChunkStats){ println!("Artifact:                    {}\nChunks:                      {}\nLogical bytes:               {}\nMin / median / max:          {} / {:.0} / {}\nAverage bytes:               {:.0}\nDuplicate chunk instances:   {}\nDuplicate logical bytes:     {}\nChunks compressed smaller:   {}\nChunks preferring raw:       {}\nBest full encoded bytes:     {}\nCompression savings:         {:.2}%", r.artifact,r.chunks,r.logical_bytes,r.min_bytes,r.median_bytes,r.max_bytes,r.average_bytes,r.exact_duplicate_chunks,r.exact_duplicate_bytes,r.compressed_smaller_chunks,r.raw_preferred_chunks,r.full_encoded_bytes,r.compression_savings_pct); }

fn simulate_policy_cmd(left:&Path,right:&Path,chunk_sizes:&[usize],thresholds:&[u8],level:i32,json:bool)->anyhow::Result<()> {
    anyhow::ensure!(!chunk_sizes.is_empty(),"at least one chunk size is required"); anyhow::ensure!(!thresholds.is_empty(),"at least one delta threshold is required"); anyhow::ensure!(thresholds.iter().all(|v|*v<=100),"delta thresholds must be <= 100");
    let rows=simulate_policy(left,right,chunk_sizes,thresholds,level)?; if json { println!("{}",serde_json::to_string_pretty(&rows)?); return Ok(()); }
    println!("ModelVault policy simulation\nLeft:  {}\nRight: {}\n\n{:>12} {:>10} {:>10} {:>10} {:>18} {:>12} {:>10}",left.display(),right.display(),"Chunk","Delta","Chunks","Shared","Estimated physical","Net save","Time ms"); println!("{}","-".repeat(94));
    for r in rows { println!("{:>12} {:>9}% {:>10} {:>10} {:>18} {:>11.2}% {:>10}",r.chunk_size,r.delta_threshold_pct,r.chunks,r.shared_chunks,r.estimated_physical_bytes,r.net_savings_pct,r.elapsed_ms); }
    Ok(())
}
