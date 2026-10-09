param(
    [string]$PrivateKeyPath = (Join-Path (Split-Path -Parent $PSScriptRoot) '.local/updater.key'),
    [switch]$Upload,
    [string]$GitHubCliPath = 'gh'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Split-Path -Parent $PSScriptRoot
$config = Get-Content -Raw -LiteralPath (Join-Path $root 'apps/desktop/tauri.conf.json') | ConvertFrom-Json
if (-not (Test-Path -LiteralPath $PrivateKeyPath -PathType Leaf)) { throw 'Restore the matching updater private key from its secure backup before uploading it.' }
$publicKey = (Get-Content -Raw -LiteralPath "$PrivateKeyPath.pub").Trim()
if ($publicKey -cne $config.plugins.updater.pubkey) { throw 'The local keypair differs from the public key embedded in the desktop. Do not rotate an established updater key implicitly.' }
if ($Upload) {
    $existing = & $GitHubCliPath secret list --repo erik-anderson/transmog --env release-signing --json name | ConvertFrom-Json
    if (@($existing | Where-Object name -CEQ 'TRANSMOG_UPDATER_PRIVATE_KEY').Count) { throw 'The updater secret already exists. Its value cannot be read back; verify key ownership before replacing it.' }
    Get-Content -Raw -LiteralPath $PrivateKeyPath | & $GitHubCliPath secret set TRANSMOG_UPDATER_PRIVATE_KEY --repo erik-anderson/transmog --env release-signing
    if ($LASTEXITCODE -ne 0) { throw 'GitHub did not accept the protected updater signing secret.' }
    Write-Host 'Configured TRANSMOG_UPDATER_PRIVATE_KEY in the protected release-signing environment.'
} else {
    Write-Host 'The local updater keypair matches the public key embedded in the desktop. Use -Upload with an authenticated GitHub CLI to configure release signing.'
}
