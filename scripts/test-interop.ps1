[CmdletBinding()]
param(
    [switch]$SkipBrowserInstall
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. "$PSScriptRoot\dev-env.ps1"

function Resolve-DockerCli {
    $command = Get-Command docker -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    $candidates = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\resources\bin\docker.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Docker\Docker\resources\bin\docker.exe'),
        (Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    throw 'Docker CLI was not found. Install Docker Desktop or add docker to PATH.'
}

function Resolve-CurlCli {
    foreach ($name in @('curl.exe', 'curl')) {
        $command = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue
        if ($null -ne $command) {
            return $command.Source
        }
    }
    throw 'A standalone curl executable was not found on PATH.'
}

function Get-PublishedUrl(
    [string]$DockerCli,
    [string]$ComposeFile,
    [string]$Project,
    [string]$Service
) {
    $binding = (& $DockerCli compose --project-name $Project --file $ComposeFile port $Service 80 |
        Select-Object -First 1).Trim()
    if ($binding -notmatch '127\.0\.0\.1:(?<port>[0-9]+)$') {
        throw "Unexpected loopback port binding for ${Service}: $binding"
    }
    return "http://127.0.0.1:$($Matches.port)/"
}

function Get-FreeTcpUdpPort {
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        $tcp = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
        $udp = $null
        try {
            $tcp.Start()
            $port = ([Net.IPEndPoint]$tcp.LocalEndpoint).Port
            $udp = [Net.Sockets.UdpClient]::new()
            $udp.Client.Bind([Net.IPEndPoint]::new([Net.IPAddress]::Loopback, $port))
            return $port
        } catch {
            continue
        } finally {
            if ($null -ne $udp) {
                $udp.Dispose()
            }
            $tcp.Stop()
        }
    }
    throw 'Could not reserve a matching loopback TCP/UDP port for the HTTP/3 fixture.'
}

$dockerCli = Resolve-DockerCli
$curlCli = Resolve-CurlCli
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$composeFile = Join-Path $repoRoot 'e2e\interop\compose.yaml'
$playwrightProject = Join-Path $repoRoot 'e2e\playwright'
$interopRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot '.local\interop'))
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = [IO.Path]::GetFullPath((Join-Path $interopRoot $runId))
$project = "rustymiddle-interop-$($runId.Substring(0, 12))"
$binaryName = if ($IsWindows) { 'rustymiddle.exe' } else { 'rustymiddle' }
$binary = Join-Path $repoRoot "target\release\$binaryName"
$caCertificate = Join-Path $runRoot 'ca.pem'
$caPrivateKey = Join-Path $runRoot 'ca.key'
$originCertificate = Join-Path $runRoot 'origin.pem'
$originPrivateKey = Join-Path $runRoot 'origin.key'
$assetsDirectory = Join-Path $runRoot 'encoded'
$composeAttempted = $false
$composeStarted = $false

if (-not $runRoot.StartsWith($interopRoot + [IO.Path]::DirectorySeparatorChar)) {
    throw "Refusing to use an interoperability run directory outside $interopRoot"
}

New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
try {
    cargo build --release --locked -p rustymiddle
    & $binary ca generate `
        --cert $caCertificate `
        --key $caPrivateKey `
        --name "rustymiddle interop $($runId.Substring(0, 12))"
    & $binary ca issue `
        --ca-cert $caCertificate `
        --ca-key $caPrivateKey `
        --identity '127.0.0.1' `
        --cert $originCertificate `
        --key $originPrivateKey `
        --days 7

    node (Join-Path $repoRoot 'e2e\interop\generate-encoded-fixtures.mjs') $assetsDirectory

    $env:RUSTYMIDDLE_INTEROP_ASSETS_DIR = $assetsDirectory
    $env:RUSTYMIDDLE_INTEROP_TLS_DIR = $runRoot
    $env:RUSTYMIDDLE_INTEROP_TLS_PORT = [string](Get-FreeTcpUdpPort)

    $composeAttempted = $true
    & $dockerCli compose `
        --project-name $project `
        --file $composeFile `
        up --detach --pull missing --wait --wait-timeout 120
    $composeStarted = $true

    $env:RUSTYMIDDLE_BIN = $binary
    $env:RUSTYMIDDLE_TEST_CA_CERT = $caCertificate
    $env:RUSTYMIDDLE_TEST_CA_KEY = $caPrivateKey
    $env:RUSTYMIDDLE_CURL = $curlCli
    $env:RUSTYMIDDLE_NGINX_URL = Get-PublishedUrl $dockerCli $composeFile $project 'nginx'
    $env:RUSTYMIDDLE_APACHE_URL = Get-PublishedUrl $dockerCli $composeFile $project 'apache'
    $env:RUSTYMIDDLE_CADDY_URL = "https://127.0.0.1:$($env:RUSTYMIDDLE_INTEROP_TLS_PORT)/"
    $env:RUSTYMIDDLE_INTEROP = '1'

    Write-Output "NGINX_ORIGIN=$($env:RUSTYMIDDLE_NGINX_URL)"
    Write-Output "APACHE_ORIGIN=$($env:RUSTYMIDDLE_APACHE_URL)"
    Write-Output "CADDY_ORIGIN=$($env:RUSTYMIDDLE_CADDY_URL)"
    Write-Output "CURL=$curlCli"
    Write-Output "CURL_VERSION=$((& $curlCli --version | Select-Object -First 1).Trim())"

    Push-Location $playwrightProject
    try {
        npm ci
        if (-not $SkipBrowserInstall) {
            npx playwright install chromium
        }
        npm run test:interop
    } finally {
        Pop-Location
    }
} catch {
    if ($composeStarted) {
        & $dockerCli compose `
            --project-name $project `
            --file $composeFile `
            logs --no-color
    }
    throw
} finally {
    if ($composeAttempted) {
        $oldNativePreference = $PSNativeCommandUseErrorActionPreference
        try {
            $PSNativeCommandUseErrorActionPreference = $false
            try {
                & $dockerCli compose `
                    --project-name $project `
                    --file $composeFile `
                    down --volumes --remove-orphans
                if ($LASTEXITCODE -ne 0) {
                    Write-Warning "Docker Compose cleanup failed with exit code $LASTEXITCODE."
                }
            } catch {
                Write-Warning "Docker Compose cleanup could not run: $($_.Exception.Message)"
            }
        } finally {
            $PSNativeCommandUseErrorActionPreference = $oldNativePreference
        }
    }
    if (Test-Path -LiteralPath $runRoot) {
        Remove-Item -LiteralPath $runRoot -Recurse -Force
    }
    if ((Test-Path -LiteralPath $interopRoot) -and
        -not (Get-ChildItem -LiteralPath $interopRoot -Force | Select-Object -First 1)) {
        Remove-Item -LiteralPath $interopRoot -Force
    }
}
