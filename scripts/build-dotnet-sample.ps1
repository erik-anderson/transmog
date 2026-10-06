[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Assert-LastCommand([string]$Description) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$project = Join-Path $repoRoot 'examples\dotnet-embedding\Transmog.Embedding.Sample\Transmog.Embedding.Sample.csproj'
$cargoArguments = @('build', '--locked', '-p', 'transmog-dotnet-sample-native')
$cargoProfile = 'debug'
if ($Configuration -eq 'Release') {
    $cargoArguments += '--release'
    $cargoProfile = 'release'
}

if ($IsWindows) {
    . (Join-Path $PSScriptRoot 'dev-env.ps1')
    $nativeName = 'transmog_dotnet_sample_native.dll'
}
elseif ($IsLinux) {
    $nativeName = 'libtransmog_dotnet_sample_native.so'
}
elseif ($IsMacOS) {
    $nativeName = 'libtransmog_dotnet_sample_native.dylib'
}
else {
    throw 'The sample build script does not recognize this operating system.'
}

Push-Location $repoRoot
try {
    & cargo @cargoArguments
    Assert-LastCommand 'Building the native Transmog bridge'

    & dotnet build $project --configuration $Configuration
    Assert-LastCommand 'Building the .NET embedding sample'

    $nativeSource = Join-Path $repoRoot "target\$cargoProfile\$nativeName"
    $managedOutput = Join-Path (Split-Path $project) "bin\$Configuration\net10.0"
    if (-not (Test-Path -LiteralPath $nativeSource -PathType Leaf)) {
        throw "Native bridge output was not found at $nativeSource"
    }
    Copy-Item -LiteralPath $nativeSource -Destination $managedOutput -Force
    Write-Output "SAMPLE_OUTPUT=$managedOutput"
}
finally {
    Pop-Location
}
