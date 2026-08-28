[CmdletBinding()]
param(
    [string]$Pointer = 'models\all-MiniLM-L6-v2\model.safetensors.mvptr',
    [string]$MinioImage = 'minio/minio:latest',
    [string]$MinioClientImage = 'minio/mc:latest',
    [switch]$AllowDirty,
    [switch]$KeepArtifacts
)

$ErrorActionPreference = 'Stop'

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock]$Command,
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

function Test-DockerImage {
    param([Parameter(Mandatory = $true)][string]$Image)

    & docker image inspect $Image *> $null
    return $LASTEXITCODE -eq 0
}

function Wait-MinIOHealth {
    param([Parameter(Mandatory = $true)][string]$Endpoint)

    for ($attempt = 1; $attempt -le 30; $attempt++) {
        try {
            $response = Invoke-WebRequest -Uri "$Endpoint/minio/health/live" -SkipHttpErrorCheck
            if ($response.StatusCode -eq 200) {
                return
            }
        }
        catch {
            # The container may still be starting.
        }
        Start-Sleep -Seconds 1
    }
    throw "Timed out waiting for MinIO at $Endpoint."
}

$repo = Split-Path -Parent $PSScriptRoot
$runId = [Guid]::NewGuid().ToString('N')
$container = "modelvault-minio-$runId"
$bucket = "modelvault-test-$runId"
$remoteName = "minio-acceptance-$runId"
$prefix = 'modelvault-acceptance'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "modelvault-minio-$runId"
$clone = Join-Path $testRoot 'clone'
$sourceConfig = Join-Path $repo '.modelvault\config.json'
$sourceConfigExisted = Test-Path -LiteralPath $sourceConfig
$minioImageExisted = $false
$minioClientImageExisted = $false
$containerStarted = $false
$sourceRemoteAdded = $false
$cloneRemoteAdded = $false
$endpoint = $null

$originalEnvironment = @{}
foreach ($name in 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_REGION', 'AWS_EC2_METADATA_DISABLED') {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

Push-Location $repo
try {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw 'Docker CLI is required. Install Docker Desktop and ensure its Linux engine is running.'
    }
    Invoke-Checked -Description 'Docker daemon check' -Command { docker version --format '{{.Server.Version}}' }
    Invoke-Checked -Description 'Git working-tree check' -Command { git rev-parse --is-inside-work-tree }
    if ((git status --porcelain) -and -not $AllowDirty) {
        throw 'The source repository must be clean before creating the acceptance clone.'
    }
    if (git status --porcelain) {
        Write-Warning 'The source repository is dirty; the clean clone tests committed HEAD while source-side commands test the current working tree.'
    }
    if (-not (Test-Path -LiteralPath $Pointer)) {
        throw "Pointer not found: $Pointer"
    }

    $minioImageExisted = Test-DockerImage -Image $MinioImage
    $minioClientImageExisted = Test-DockerImage -Image $MinioClientImage

    $accessKey = "modelvault$($runId.Substring(0, 12))"
    $secretKey = "mv$runId"
    $env:AWS_ACCESS_KEY_ID = $accessKey
    $env:AWS_SECRET_ACCESS_KEY = $secretKey
    $env:AWS_REGION = 'us-east-1'
    $env:AWS_EC2_METADATA_DISABLED = 'true'

    Write-Host "Starting temporary MinIO container: $container"
    Invoke-Checked -Description 'MinIO container start' -Command {
        docker run --detach --name $container --publish 127.0.0.1::9000 `
            --env "MINIO_ROOT_USER=$accessKey" --env "MINIO_ROOT_PASSWORD=$secretKey" `
            $MinioImage server /data
    }
    $containerStarted = $true

    $portMapping = (docker port $container 9000/tcp | Select-Object -First 1)
    if (-not $portMapping -or $portMapping -notmatch ':(\d+)$') {
        throw "Unable to determine the loopback port for MinIO container $container."
    }
    $hostPort = $Matches[1]
    $endpoint = "http://127.0.0.1:$hostPort"
    Wait-MinIOHealth -Endpoint $endpoint

    $mcAlias = "http://$accessKey`:$secretKey@host.docker.internal:$hostPort"
    Invoke-Checked -Description 'MinIO bucket creation' -Command {
        docker run --rm --env "MC_HOST_local=$mcAlias" $MinioClientImage mb --ignore-existing "local/$bucket"
    }

    Invoke-Checked -Description 'Source MinIO remote configuration' -Command {
        cargo run --locked --features s3 -- remote add-minio $remoteName $bucket $endpoint --prefix $prefix
    }
    $sourceRemoteAdded = $true
    Invoke-Checked -Description 'Deep-verified S3 push' -Command {
        cargo run --locked --features s3 -- push $Pointer --remote-name $remoteName --deep-verify
    }

    New-Item -ItemType Directory -Path $testRoot | Out-Null
    Invoke-Checked -Description 'Clean Git clone' -Command { git clone --no-local $repo $clone }

    Push-Location $clone
    try {
        Invoke-Checked -Description 'Clone MinIO remote configuration' -Command {
            cargo run --locked --features s3 -- remote add-minio $remoteName $bucket $endpoint --prefix $prefix
        }
        $cloneRemoteAdded = $true
        Invoke-Checked -Description 'Deep-verified S3 pull' -Command {
            cargo run --locked --features s3 -- pull $Pointer --remote-name $remoteName --deep-verify
        }
        Invoke-Checked -Description 'Artifact checkout' -Command {
            cargo run --locked --features s3 -- checkout $Pointer
        }
        Invoke-Checked -Description 'Deep repository fsck' -Command {
            cargo run --locked --features s3 -- fsck --deep
        }
    }
    finally {
        if ($cloneRemoteAdded) {
            & cargo run --locked --features s3 -- remote remove $remoteName
        }
        Pop-Location
    }

    Write-Host "MinIO S3 acceptance test passed. Endpoint: $endpoint; bucket: $bucket"
}
finally {
    if ($sourceRemoteAdded) {
        & cargo run --locked --features s3 -- remote remove $remoteName
    }
    if (-not $sourceConfigExisted -and (Test-Path -LiteralPath $sourceConfig)) {
        Remove-Item -LiteralPath $sourceConfig
    }
    foreach ($name in $originalEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], 'Process')
    }

    if (-not $KeepArtifacts) {
        if (Test-Path -LiteralPath $testRoot) {
            Remove-Item -LiteralPath $testRoot -Recurse -Force
        }
        if ($containerStarted) {
            & docker rm --force $container *> $null
        }
        if (-not $minioClientImageExisted) {
            & docker image rm $MinioClientImage *> $null
        }
        if (-not $minioImageExisted) {
            & docker image rm $MinioImage *> $null
        }
    }
}
