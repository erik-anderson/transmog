$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
Add-Type -Path (Join-Path $repositoryRoot 'apps\desktop\windows\loopback.cs')
$a='S-1-15-2-1'; $b='S-1-15-2-2'; $c='S-1-15-2-3'; $stale='S-1-15-2-4'
$current = [Collections.Generic.Dictionary[string,uint32]]::new([StringComparer]::Ordinal)
$current.Add($a,7); $current.Add($b,12); $current.Add($stale,0)
$installed = [Collections.Generic.HashSet[string]]::new([string[]]@($a,$b,$c),[StringComparer]::Ordinal)
$enabled = [Transmog.Loopback]::Merge($current,[string[]]@($a,$c),$true,$installed)
if ($enabled.Count -ne 4 -or $enabled[$a] -ne 7 -or $enabled[$b] -ne 12 -or $enabled[$stale] -ne 0 -or $enabled[$c] -ne 0) { throw 'Enabling a selection discarded an unrelated exemption or its attributes.' }
$removed = [Transmog.Loopback]::Merge($current,[string[]]@($b,$stale),$false,$installed)
if ($removed.Count -ne 1 -or $removed[$a] -ne 7 -or $current.Count -ne 3) { throw 'Removal changed an unselected entry or the original configuration.' }
foreach ($selection in @(@($a,$a),@('S-1-5-18'),@($stale))) {
    $rejected = $false
    try { [Transmog.Loopback]::Merge($current,[string[]]$selection,$true,$installed) | Out-Null }
    catch { $rejected = $true }
    if (-not $rejected) { throw 'Invalid, duplicate, or uninstalled additions were accepted.' }
}
# Only read the real Windows state. All write planning above uses fixture SIDs.
$inventory = @([Transmog.Loopback]::Read())
if (@($inventory.sid | Sort-Object -Unique).Count -ne $inventory.Count) { throw 'Repeated per-user AppContainers were not grouped by SID.' }
foreach ($row in $inventory) {
    if (-not $row.displayName -or $row.sid -notmatch '^S-1-15-2-' -or $null -eq $row.users) { throw 'The inventory omitted an AppContainer identity or users.' }
}
Write-Host "Exemption preservation, attributes, multi-selection, stale removal and invalid additions passed. Read-only Windows inventory: $($inventory.Count) AppContainers, $(@($inventory | Where-Object exempt).Count) enabled."
