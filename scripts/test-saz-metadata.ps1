[CmdletBinding()]
param([Parameter(Mandatory)][string]$FixtureDirectory)
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.IO.Compression.FileSystem
$requiredDates = @('ClientConnected','ClientDoneRequest','ServerGotRequest','ServerDoneResponse','ClientBeginResponse','ClientDoneResponse')
$dateAttributes = $requiredDates + @('ClientBeginRequest','GotRequestHeaders','ServerConnected','FiddlerBeginRequest','ServerBeginResponse','GotResponseHeaders')
$archives = @(Get-ChildItem -LiteralPath $FixtureDirectory -Filter '*-plain.saz' -File)
if (-not $archives.Count) { throw 'No unencrypted SAZ fixtures were found.' }

function Read-MemberBytes($Archive, [string]$Name) {
    $entry = $Archive.GetEntry($Name)
    if (-not $entry) { throw "Missing archive member: $Name" }
    $stream = $entry.Open()
    $output = [IO.MemoryStream]::new()
    try {
        $stream.CopyTo($output)
        return ,$output.ToArray()
    } finally {
        $stream.Dispose()
        $output.Dispose()
    }
}

$checked = 0
foreach ($file in $archives) {
    $archive = [IO.Compression.ZipFile]::OpenRead($file.FullName)
    try {
        $members = @($archive.Entries | Where-Object { $_.FullName -match '^raw/[0-9]+_m\.xml$' })
        if (-not $members.Count) { throw "No session metadata in $($file.Name)." }
        foreach ($member in $members) {
            $bytes = Read-MemberBytes $archive $member.FullName
            $reader = [Xml.XmlTextReader]::new([IO.MemoryStream]::new($bytes, $false))
            $foundTimers = $false
            try {
                while ($reader.Read()) {
                    if ($reader.NodeType -ne [Xml.XmlNodeType]::Element -or $reader.Name -ne 'SessionTimers') { continue }
                    $foundTimers = $true
                    foreach ($name in $dateAttributes) {
                        $value = $reader.GetAttribute($name)
                        if ($null -eq $value) {
                            if ($requiredDates -contains $name) { throw "$($file.Name)/$($member.FullName): required timer $name is missing." }
                            continue
                        }
                        [Xml.XmlConvert]::ToDateTime($value, [Xml.XmlDateTimeSerializationMode]::RoundtripKind) | Out-Null
                    }
                }
            } finally { $reader.Dispose() }
            if (-not $foundTimers) { throw "No SessionTimers in $($member.FullName)." }
            $checked++
        }
    } finally { $archive.Dispose() }
}
Write-Host "$checked sessions in $($archives.Count) SAZ fixtures verified with .NET XML date parsing."
