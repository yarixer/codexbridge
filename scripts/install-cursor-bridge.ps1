param(
    [string]$Version = "1.0.31"
)

$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "Use scripts/install-cursor-bridge.sh on Linux."
}

if ($env:PROCESSOR_ARCHITECTURE -notin @("AMD64", "x86_64")) {
    throw "Cursor SDK Bridge supports Windows x64 only."
}

$archiveName = "cursor-sdk-bridge-standalone-win32-x64.tar.gz"
$expectedSha256 = "7121271f4dc4802d16530e25446df60361a5432adf69db454785131139e63ce9"
$downloadUrl = "https://github.com/cursor/sdk-bridge/releases/download/v$Version/$archiveName"
$projectRoot = Split-Path -Parent $PSScriptRoot
$installRoot = Join-Path $projectRoot ".tools\cursor-sdk-bridge"
$archivePath = Join-Path ([System.IO.Path]::GetTempPath()) "codexbridge-$archiveName"

Write-Host "Downloading Cursor SDK Bridge v$Version..."
Invoke-WebRequest -Uri $downloadUrl -OutFile $archivePath
$actualSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualSha256 -ne $expectedSha256) {
    throw "SHA-256 mismatch for $archiveName. Expected $expectedSha256, got $actualSha256."
}

New-Item -ItemType Directory -Force -Path $installRoot | Out-Null
tar -xzf $archivePath -C $installRoot
Remove-Item -LiteralPath $archivePath

$binary = Join-Path $installRoot "bin\cursor-sdk-bridge.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Archive did not contain $binary."
}

& $binary --help | Select-Object -First 1
Write-Host "Installed: $binary"

