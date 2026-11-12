param(
    [Parameter(Mandatory)][string]$Path,
    [switch]$Snapshot,
    [string]$ExistingWindows = ''
)

$ErrorActionPreference = 'Stop'
$shell = New-Object -ComObject Shell.Application
if ($Snapshot) {
    $handles = @($shell.Windows() | ForEach-Object { [string]$_.HWND })
    ConvertTo-Json -InputObject $handles -Compress
    return
}
$expected = [IO.Path]::GetFullPath($Path)
$priorHandles = $ExistingWindows.Split(',')
$deadline = [DateTime]::UtcNow.AddSeconds(15)
while ([DateTime]::UtcNow -lt $deadline) {
    foreach ($window in @($shell.Windows())) {
        try {
            $selected = @($window.Document.SelectedItems() | ForEach-Object { $_.Path })
            if ($selected -contains $expected) {
                # Only close an Explorer window created by this isolated check.
                if ([string]$window.HWND -notin $priorHandles) { $window.Quit() }
                Write-Output 'Saved recording selected in File Explorer.'
                return
            }
        } catch {
            # Explorer may still be navigating or initializing its document.
        }
    }
    Start-Sleep -Milliseconds 100
}
throw 'File Explorer did not select the saved recording.'
