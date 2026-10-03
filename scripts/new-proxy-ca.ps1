param(
    [Parameter(Mandatory = $true)]
    [string]$CertificatePath,
    [Parameter(Mandatory = $true)]
    [string]$PrivateKeyPath,
    [string]$Name = "rustymiddle local interception CA",
    [string]$BinaryPath
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
. "$PSScriptRoot\dev-env.ps1"

$certificate = [System.IO.Path]::GetFullPath($CertificatePath)
$privateKey = [System.IO.Path]::GetFullPath($PrivateKeyPath)
if ($BinaryPath) {
    $binary = (Resolve-Path -LiteralPath $BinaryPath).Path
    & $binary ca generate --cert $certificate --key $privateKey --name $Name
} else {
    cargo run --locked -p rustymiddle -- ca generate --cert $certificate --key $privateKey --name $Name
}

$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$acl = [System.Security.AccessControl.FileSecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
    $identity,
    [System.Security.AccessControl.FileSystemRights]::Read -bor
        [System.Security.AccessControl.FileSystemRights]::Write,
    [System.Security.AccessControl.AccessControlType]::Allow
)
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $privateKey -AclObject $acl
Write-Host "Applied a user-only ACL to $privateKey"
