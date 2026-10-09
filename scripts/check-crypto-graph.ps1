$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
# Evaluate actual target graphs: unfiltered metadata can combine a Windows-only
# owner with a dependency's mobile-only edges into an impossible runtime path.
foreach ($graphTarget in @('x86_64-pc-windows-msvc','x86_64-unknown-linux-gnu','aarch64-apple-darwin')) {
    $metadata = cargo metadata --format-version 1 --locked --filter-platform $graphTarget | ConvertFrom-Json -AsHashtable
    $packagesById = @{}
    $nodesById = @{}
    foreach ($package in $metadata.packages) { $packagesById[$package.id] = $package }
    foreach ($node in $metadata.resolve.nodes) { $nodesById[$node.id] = $node }
    $normalIds = [System.Collections.Generic.HashSet[string]]::new()
    $queue = [System.Collections.Generic.Queue[string]]::new()
    foreach ($member in $metadata.workspace_members) { $queue.Enqueue($member) }
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        if (-not $normalIds.Add($id)) { continue }
        foreach ($dependency in $nodesById[$id].deps) {
            # Only this desktop-owned SDK edge is exempt. A reference from any
            # other production root still enters the checked graph below.
            if ($packagesById[$id].name -ceq 'transmog-desktop' -and $packagesById[$dependency.pkg].name -ceq 'tauri-plugin-updater') { continue }
            if ($dependency.dep_kinds | Where-Object { $null -eq $_.kind -or $_.kind -eq 'normal' }) { $queue.Enqueue($dependency.pkg) }
        }
    }
    $packages = $metadata.packages | Where-Object { $normalIds.Contains($_.id) }
    $forbidden = @('openssl', 'openssl-sys', 'native-tls', 'rustls', 'rustls-webpki', 'webpki')
    $foundForbidden = $packages | Where-Object { $forbidden -contains $_.name }
    if ($foundForbidden) { throw "Forbidden production TLS crates on $graphTarget : $(($foundForbidden.name | Sort-Object -Unique) -join ', ')" }
    foreach ($name in @('boring', 'boring-sys')) {
        $versions = @($packages | Where-Object { $_.name -eq $name } | ForEach-Object { $_.version } | Sort-Object -Unique)
        if ($versions.Count -ne 1) { throw "Expected exactly one $name version on $graphTarget; found: $($versions -join ', ')" }
    }
    Write-Host "$graphTarget : proxy/replay graphs contain one BoringSSL family; the desktop updater is the only isolated HTTPS-client exception."
}
