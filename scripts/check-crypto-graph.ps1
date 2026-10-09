$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json -AsHashtable
$normalIds = [System.Collections.Generic.HashSet[string]]::new()
$queue = [System.Collections.Generic.Queue[string]]::new()
foreach ($member in $metadata.workspace_members) { $queue.Enqueue($member) }
while ($queue.Count -gt 0) {
    $id = $queue.Dequeue()
    if (-not $normalIds.Add($id)) { continue }
    $node = $metadata.resolve.nodes | Where-Object id -eq $id
    foreach ($dependency in $node.deps) {
        $hasNormal = $dependency.dep_kinds | Where-Object { $null -eq $_.kind -or $_.kind -eq 'normal' }
        if ($hasNormal) { $queue.Enqueue($dependency.pkg) }
    }
}
$packages = $metadata.packages | Where-Object { $normalIds.Contains($_.id) }
$forbidden = @('openssl', 'openssl-sys', 'native-tls', 'rustls', 'rustls-webpki', 'webpki')
$foundForbidden = $packages | Where-Object { $forbidden -contains $_.name }
if ($foundForbidden) {
    throw "Forbidden production TLS crates: $(($foundForbidden.name | Sort-Object -Unique) -join ', ')"
}
foreach ($name in @('boring', 'boring-sys')) {
    $versions = @($packages | Where-Object name -eq $name | Select-Object -ExpandProperty version -Unique)
    if ($versions.Count -ne 1) {
        throw "Expected exactly one $name version in the normal graph; found: $($versions -join ', ')"
    }
}
Write-Host 'Normal dependency graph contains one BoringSSL family and no second TLS stack.'
