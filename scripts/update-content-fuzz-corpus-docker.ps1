[CmdletBinding()]
param(
    [ValidateRange(1, 1000000000)]
    [int]$Runs = 100000,

    [ValidateSet('content_encoding', 'decode_stream', 'codec_roundtrip', 'content_pipeline')]
    [string[]]$Target = @(
        'content_encoding',
        'decode_stream',
        'codec_roundtrip',
        'content_pipeline'
    ),

    [string]$Image = 'transmog-content-fuzz:nightly-2026-10-01'
)

$ErrorActionPreference = 'Stop'

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
Assert-LastCommand 'Building the content-fuzz image'

$repositoryMount = "type=bind,source=$repoRoot,target=/work"
& $dockerCli run --rm `
    --env "FUZZ_RUNS=$Runs" `
    --env "FUZZ_TARGETS=$($Target -join ' ')" `
    --mount $repositoryMount `
    --mount 'type=volume,source=transmog-content-fuzz-cargo-registry,target=/usr/local/cargo/registry' `
    --mount 'type=volume,source=transmog-content-fuzz-target,target=/work/fuzz/target' `
    --workdir /work `
    $Image `
    bash fuzz/update-corpus.sh
Assert-LastCommand 'Updating the minimized content fuzz corpora'
