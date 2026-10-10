# Thin PowerShell integration for portable Python development commands.
# TRANSMOG_PYTHON selects an explicit interpreter, including paths with spaces.
$ErrorActionPreference = 'Stop'

function Get-TransmogPython {
    if ($env:TRANSMOG_PYTHON) {
        return $env:TRANSMOG_PYTHON
    }
    $candidates = @((Get-Command python,python3,py -All -CommandType Application -ErrorAction SilentlyContinue).Source)
    $candidates += Join-Path ([Environment]::GetFolderPath('UserProfile')) 'AppData/Local/Python/bin/python.exe'
    foreach ($candidate in $candidates | Select-Object -Unique) {
        if (-not $candidate) { continue }
        try {
            $version = & $candidate -c 'import sys; print(sys.version_info >= (3, 11))' 2>$null
            if ($LASTEXITCODE -eq 0 -and $version -eq 'True') { return $candidate }
        } catch { continue }
    }
    throw 'Python 3.11+ is mandatory. Install Python or set TRANSMOG_PYTHON to its executable.'
}

function Invoke-TransmogPython([string]$Script, [object[]]$Arguments = @()) {
    & (Get-TransmogPython) (Join-Path $PSScriptRoot $Script) @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Script failed with exit code $LASTEXITCODE." }
}
