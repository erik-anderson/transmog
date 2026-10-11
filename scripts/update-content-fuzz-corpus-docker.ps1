# Convenience entry point; implementation is portable Python.
[CmdletBinding()]
param([ValidateRange(1,1000000000)][int]$Runs = 100000, [ValidateSet('content_encoding','decode_stream','codec_roundtrip','content_pipeline','websocket_frames','capture_reader','capture_crypto')][string[]]$Target = @('content_encoding','decode_stream','codec_roundtrip','content_pipeline','websocket_frames','capture_reader','capture_crypto'), [string]$Image = 'transmog-content-fuzz:nightly-2026-10-01')
. (Join-Path $PSScriptRoot 'python-tools.ps1')
$arguments = @('--runs', $Runs, '--image', $Image, '--target') + $Target
Invoke-TransmogPython 'update-content-fuzz-corpus-docker.py' $arguments
