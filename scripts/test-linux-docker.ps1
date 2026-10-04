[CmdletBinding()]
param(
    [string]$Image = 'rustymiddle-linux-tools:rust-1.97.1-nightly-2026-10-01'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Resolve-DockerCli {
    $command = Get-Command docker -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    $candidates = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\resources\bin\docker.exe'),
        (Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    throw 'Docker CLI was not found. Install Docker Desktop or add docker to PATH.'
}

function Assert-LastCommand([string]$Description) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

$dockerCli = Resolve-DockerCli
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dockerfile = Join-Path $repoRoot 'fuzz\Dockerfile'
$buildContext = Join-Path $repoRoot 'fuzz'

& $dockerCli build --file $dockerfile --tag $Image $buildContext
Assert-LastCommand 'Building the Linux validation image'

$repositoryMount = "type=bind,source=$repoRoot,target=/work,readonly"
& $dockerCli run --rm `
    --env 'CFLAGS=' `
    --env 'CXXFLAGS=' `
    --env 'CARGO_TARGET_DIR=/work-target' `
    --mount $repositoryMount `
    --mount 'type=volume,source=rustymiddle-linux-cargo-registry,target=/usr/local/cargo/registry' `
    --mount 'type=volume,source=rustymiddle-linux-target,target=/work-target' `
    --workdir /work `
    $Image `
    bash -c 'set -euo pipefail; cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --all-features --all-targets --locked && cargo build --release --workspace --all-features --locked'
Assert-LastCommand 'Running the Linux workspace validation matrix'
