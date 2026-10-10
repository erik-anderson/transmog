# Convenience entry point for checking or uploading the established updater key.
param([string]$PrivateKeyPath = (Join-Path (Split-Path -Parent $PSScriptRoot) '.local/updater.key'), [switch]$Upload, [string]$GitHubCliPath = 'gh')
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--private-key-path', $PrivateKeyPath, '--github-cli-path', $GitHubCliPath)
if ($Upload) { $arguments += '--upload' }
Invoke-TransmogPython 'setup-updater-signing.py' $arguments
