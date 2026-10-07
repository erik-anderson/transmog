param([Parameter(Mandatory)][string]$ClientId, [Parameter(Mandatory)][string]$TenantId, [Parameter(Mandatory)][string]$ReportPath)
$ErrorActionPreference = 'Stop'
foreach ($id in @($ClientId, $TenantId)) { if ($id -notmatch '^[0-9a-f-]{36}$') { throw 'Invalid Azure identifier in probe configuration.' } }
if (-not $env:ACTIONS_ID_TOKEN_REQUEST_URL -or -not $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN) { throw 'The diagnostic job must have an OIDC token to test Azure rejection.' }
$separator = if ($env:ACTIONS_ID_TOKEN_REQUEST_URL.Contains('?')) { '&' } else { '?' }
$oidc = Invoke-RestMethod -Uri ($env:ACTIONS_ID_TOKEN_REQUEST_URL + $separator + 'audience=api%3A%2F%2FAzureADTokenExchange') -Headers @{ Authorization = "Bearer $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN" }
$response = Invoke-WebRequest -Method Post -Uri "https://login.microsoftonline.com/$TenantId/oauth2/v2.0/token" -SkipHttpErrorCheck -ContentType 'application/x-www-form-urlencoded' -Body @{
    client_id = $ClientId; scope = 'https://codesigning.azure.net/.default'; grant_type = 'client_credentials'
    client_assertion_type = 'urn:ietf:params:oauth:client-assertion-type:jwt-bearer'; client_assertion = $oidc.value
}
$oidc = $null
$errorResponse = $response.Content | ConvertFrom-Json
if ($response.StatusCode -eq 200 -or -not (@($errorResponse.error_codes) | Where-Object { $_ -in @(70021, 700213, 7002138) })) { throw 'Azure did not reject the unprotected GitHub subject with a federation mismatch.' }
$response = $null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ReportPath) | Out-Null
[ordered]@{ UnprotectedJobAuthenticationDenied = $true; AzureErrorCodes = $errorResponse.error_codes; Commit = $env:GITHUB_SHA; RunId = $env:GITHUB_RUN_ID } |
    ConvertTo-Json | Set-Content -LiteralPath $ReportPath -Encoding utf8NoBOM
Write-Host 'Azure rejected the GitHub OIDC token from outside release-signing.'
