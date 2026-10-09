param()
$ErrorActionPreference = 'Stop'
function New-Release($Version, $Id, $Prerelease = $false) {
    [pscustomobject]@{ tag_name = "v$Version"; id = $Id; draft = $false; prerelease = $Prerelease; assets = @(@{name='latest.json'}, @{name="Transmog_${Version}_x64-setup.exe.sig"}) }
}
function Invoke-RestMethod {
    param($Uri, $Headers, $Method, $ContentType, $Body)
    if ($Method -eq 'Patch') {
        if (($Body | ConvertFrom-Json).make_latest -cne 'true') { throw 'Stable pointer mutation changed unrelated release metadata.' }
        $case.Patches += $Uri; return
    }
    if ($Uri -like '*/releases/latest') { return @{id=$case.LatestId} }
    if ($Uri -like '*page=2') { Write-Output -NoEnumerate $case.SecondPage; return }
    Write-Output -NoEnumerate $case.Releases
}
foreach ($case in @(
    @{ Releases=@((New-Release '0.1.9' 9),(New-Release '0.1.10' 10),(New-Release '2.0.0' 20 $true)); SecondPage=@(); LatestId=9; Expected=10; Patches=@() },
    @{ Releases=@((New-Release '0.1.10' 10),(New-Release '0.1.9' 9)); SecondPage=@(); LatestId=10; Expected=$null; Patches=@() },
    @{ Releases=@(1..100 | ForEach-Object { New-Release "1.0.$_" $_ $true }); SecondPage=@((New-Release '0.2.0' 200)); LatestId=9; Expected=200; Patches=@() }
)) {
    & (Join-Path $PSScriptRoot 'sync-stable-update-release.ps1')
    if ($case.Expected) {
        if ($case.Patches.Count -ne 1 -or $case.Patches[0] -notlike "*/releases/$($case.Expected)") { throw 'Stable feed did not select the highest numeric stable version.' }
    } elseif ($case.Patches.Count) { throw 'An unchanged stable pointer was rewritten.' }
}
Write-Host 'Stable update selection passed: numeric ordering, prerelease exclusion, pagination, and idempotence.'
