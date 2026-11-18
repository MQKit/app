# Writes the winget manifests for a release into dist\winget\<version>\
# (called by package.ps1 -Release -InstallerUrl …; can also run on its own):
#
#   .\scripts\winget.ps1 -Version 1.0.0 -Sha256 <hash> -InstallerUrl https://…/MQKit-setup-1.0.0.exe
#
# Submitting: open a pull request to github.com/microsoft/winget-pkgs with the three
# files under manifests\s\SoftwareHive\MQKit\<version>\ (or `wingetcreate submit <folder>`).
# Then:  winget install SoftwareHive.MQKit

param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Sha256,
    [Parameter(Mandatory)][string]$InstallerUrl,
    [string]$ReleaseDate = (Get-Date -Format "yyyy-MM-dd")
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repoRoot "dist\winget\$Version"
New-Item -ItemType Directory -Force $out | Out-Null
$id = "SoftwareHive.MQKit"
# keep at the newest schema under github.com/microsoft/winget-cli/tree/master/schemas/JSON/manifests
$schema = "1.28.0"

$manifestVersion = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.$schema.schema.json

PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
"@

# Tauri's NSIS installer: per user (no admin), silent with /S.
$manifestInstaller = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.$schema.schema.json

PackageIdentifier: $id
PackageVersion: $Version
MinimumOSVersion: 10.0.17763.0
InstallerType: nullsoft
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
ReleaseDate: $ReleaseDate
Installers:
- Architecture: x64
  InstallerUrl: $InstallerUrl
  InstallerSha256: $($Sha256.ToUpperInvariant())
ManifestType: installer
ManifestVersion: $schema
"@

$manifestLocale = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.$schema.schema.json

PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: en-US
Publisher: Software Hive S.R.L.
PublisherUrl: https://softwarehive.ro/
Author: Software Hive S.R.L.
PackageName: MQKit
License: AGPL-3.0-or-later
Copyright: Copyright (C) 2026 Software Hive S.R.L.
ShortDescription: Free file converter for Windows that works offline - 44 tools for PDF, images, video, audio and documents.
Description: |-
  44 tools: compress, merge and split PDFs, PDF to Word, Excel and PowerPoint, OCR, Office files to PDF, compress, convert and resize images, HEIC to JPG, AI upscale and background removal, compress and convert video and audio, video to MP3, speech to text, e-books, archives and more. Everything runs on your PC: nothing is uploaded. Free, no ads, no account, open source.
Moniker: mqkit
Tags:
- converter
- pdf
- compress
- heic
- jpg
- mp3
- word
- offline
- open-source
ManifestType: defaultLocale
ManifestVersion: $schema
"@

$utf8 = New-Object Text.UTF8Encoding $false
[IO.File]::WriteAllText((Join-Path $out "$id.yaml"), $manifestVersion + "`n", $utf8)
[IO.File]::WriteAllText((Join-Path $out "$id.installer.yaml"), $manifestInstaller + "`n", $utf8)
[IO.File]::WriteAllText((Join-Path $out "$id.locale.en-US.yaml"), $manifestLocale + "`n", $utf8)
Write-Host "winget manifests in $out"
