# ModelVault Remotes

ModelVault supports two remote storage families:

- `filesystem` — local directories, mounted filesystems, NAS paths, and Windows UNC shares;
- `s3` — AWS S3 and S3-compatible services such as MinIO, behind the optional `s3` feature.

Remote synchronization transfers **full reconstructed logical CAS objects**. A remote does not need to understand local pack or delta representations.

## Filesystem remote

```powershell
modelvault remote add origin \\NAS01\AI\ModelVault --default
modelvault push .\models\model.safetensors.mvptr --remote-name origin
```

Direct paths remain supported:

```powershell
modelvault push .\models\model.safetensors.mvptr --remote C:\Temp\ModelVaultRemote
```

## AWS S3

ModelVault uses the AWS SDK credential provider chain. Credentials are intentionally not written to `.modelvault/config.json`.

```powershell
modelvault remote add-s3 origin my-modelvault-bucket `
    --region us-east-1 `
    --prefix team-a/modelvault `
    --default

modelvault push .\models\model.safetensors.mvptr --remote-name origin --jobs 8
```

Named profile:

```powershell
modelvault remote add-s3 origin my-modelvault-bucket `
    --region us-east-1 `
    --profile modelvault `
    --default
```

## MinIO

The convenience command configures path-style S3 addressing:

```powershell
modelvault remote add-minio lab models http://127.0.0.1:9000 `
    --prefix modelvault `
    --default
```

Equivalent generic S3 configuration:

```powershell
modelvault remote add-s3 lab models `
    --endpoint http://127.0.0.1:9000 `
    --region us-east-1 `
    --force-path-style `
    --prefix modelvault
```

Plain HTTP is appropriate for a loopback development endpoint. Use HTTPS for non-local S3-compatible endpoints. ModelVault warns when a custom non-loopback endpoint is configured with `http://`.

### Disposable MinIO acceptance test

On Windows, the repository includes an opt-in end-to-end acceptance harness for
the S3-compatible path. It requires Docker Desktop with its Linux engine
running; the first run may download the MinIO server and client images.

```powershell
.\scripts\Test-S3-MinIO.ps1
```

For the full default, S3-build, and MinIO acceptance gate in one command, run:

```powershell
.\scripts\Validate-ModelVault.ps1 -WithS3 -WithMinio
```

The script starts a loopback-only MinIO container with generated credentials,
creates a unique bucket, deep-verifies a push, then creates a clean Git clone
that deep-verifies a pull, checkout, and `fsck --deep`. It removes the unique
container, bucket data, clone, temporary remote configuration, and any MinIO
images it downloaded. `-KeepArtifacts` retains the container and clone for
failure investigation. The source repository must normally be clean; use
`-AllowDirty` only while developing the harness, because its clean clone tests
committed `HEAD` while source-side commands test the working tree.

## Logical remote layout

```text
<prefix>/
├── objects/
│   ├── 00/
│   │   └── <62 hex characters>
│   └── ff/
│       └── <62 hex characters>
└── manifests/
    └── <artifact-blake3>.json
```

## Fast vs deep verification

S3 objects uploaded by ModelVault include `x-amz-meta-modelvault-blake3`.

Normal synchronization uses **fast/backend verification** for destination-reuse decisions. For S3, a `HEAD` response containing the expected ModelVault hash metadata can satisfy that check without downloading the object. Objects without the metadata fall back to content verification.

When the remote or metadata is not fully trusted, use:

```powershell
modelvault push .\models\model.safetensors.mvptr --remote-name origin --deep-verify
modelvault pull .\models\model.safetensors.mvptr --remote-name origin --deep-verify
```

`--deep-verify` reads destination object bodies and compares BLAKE3 content before an existing object is reused. It costs additional remote reads/download bandwidth but removes the metadata-only trust shortcut.

Source objects being transferred are always checked for expected size and BLAKE3 identity before the destination accepts them.

## Retries and restartability

`--max-attempts` controls the AWS SDK retry policy for S3 operations. Default: 4.

Synchronization is restartable at CAS-object granularity. Re-running an interrupted `push` or `pull` reuses already-present valid objects and transfers only missing/corrupt ones.

## Transfer telemetry

Push/pull report wall-clock time and effective copied-byte throughput. Reused bytes are not counted as transferred bytes.
