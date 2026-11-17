# Builds the MQKit installer: dist\MQKit-setup-<version>.exe
#
#   .\scripts\package.ps1            development build: nothing is signed, no token needed;
#                                    goes to dist\dev\ and never touches a signed release
#   .\scripts\package.ps1 -Sign      a signed TEST build (no tag, no feed): to check how
#                                    antivirus programs treat the signed files; dist\dev\
#   .\scripts\package.ps1 -Release   production: from a clean, tagged tree (v<version>),
#                                    every program signed with the Software Hive certificate
#                                    (scripts\sign-windows.ps1, Certum token plugged in)
#
# Order matters: the programs are signed FIRST (worker-core,
# office-worker, the app), then packed into the installer, which is signed last.
# SmartScreen and antivirus judge each program, not only the installer.
#
# No auto-update: a release also writes dist\latest.json, the feed the app reads
# once a day to show "version X is out" with a link to the website.

param(
    [switch]$Release,
    [switch]$Sign,
    # Where the notification's button leads (a page on the website).
    [string]$ReleasePage = "",
    # Where the installer is downloaded from (for winget).
    [string]$InstallerUrl = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

function Invoke-Native([string]$What, [scriptblock]$Command) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    Push-Location $repoRoot
    try { & $Command 2>&1 | ForEach-Object { "$_" } | Out-Host } finally { $ErrorActionPreference = $saved; Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}

$version = (Get-Content (Join-Path $repoRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$cargoVersion = (Select-String -Path (Join-Path $repoRoot "Cargo.toml") -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if ($version -ne $cargoVersion) { throw "tauri.conf.json says $version but Cargo.toml says $cargoVersion." }
$dist = if ($Release) { Join-Path $repoRoot "dist" } else { Join-Path $repoRoot "dist\dev" }
$signing = $Release -or $Sign

if ($Release) {
    $dirty = git -C $repoRoot status --porcelain
    if ($dirty) { throw "The working tree has uncommitted changes. A signed release is built from a commit." }
    $tag = & { $ErrorActionPreference = "Continue"; git -C $repoRoot describe --exact-match --tags HEAD 2>$null }
    if ($tag -ne "v$version") { throw "HEAD must be tagged v$version (it is '$tag')." }
    if (-not $ReleasePage) { throw "Give -ReleasePage (the website page the update notification opens)." }
    # The notices installed with the app must match what is built now (MIT, BSD and
    # Apache require them with the program): regenerated and compared, the date aside.
    $notices = Join-Path $repoRoot "THIRD-PARTY-NOTICES.txt"
    $before = (Get-Content $notices -Raw) -replace "(?m)^Generated from .*$", ""
    Invoke-Native "generating the notices" { node scripts/generate-notices.mjs }
    $after = (Get-Content $notices -Raw) -replace "(?m)^Generated from .*$", ""
    if ($before -ne $after) { throw "THIRD-PARTY-NOTICES.txt was out of date: it is regenerated now. Commit it, tag again and rebuild." }
    git -C $repoRoot checkout -- THIRD-PARTY-NOTICES.txt
}
if ($signing) {
    $thumbprint = if ($env:MQKIT_SIGN_THUMBPRINT) { $env:MQKIT_SIGN_THUMBPRINT } else { "5DF397C236D87DA3DB0137E9CBFDF29319241E4A" }
    if (-not (Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Thumbprint -eq $thumbprint })) {
        throw "The certificate $thumbprint is not known here. Plug the token in (proCertum CardManager closed)."
    }
}

# Paths of this PC (user folder, repository) must not end up in published programs.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = @("-Ctarget-feature=+crt-static", "--remap-path-prefix=$cargoHome=cargo", "--remap-path-prefix=$repoRoot=mqkit") -join [char]0x1f
# The same for the C and C++ libraries compiled along (HarfBuzz, woff2, whisper.cpp): their
# assert() and error messages name their source files. MSVC trims those names with
# /d1trimfile; the cc and cmake crates pass CFLAGS and CXXFLAGS on to it.
$env:CFLAGS = "/d1trimfile:$repoRoot\ /d1trimfile:$cargoHome\"
$env:CXXFLAGS = $env:CFLAGS

# The DLLs a program or DLL loads at start (its import table).
function Get-DllImports([string]$File) {
    $b = [IO.File]::ReadAllBytes($File)
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    $sections = [BitConverter]::ToUInt16($b, $pe + 6)
    $optional = $pe + 24
    $optionalSize = [BitConverter]::ToUInt16($b, $pe + 20)
    # data directory 1 (imports); PE32+ has it at 120, PE32 at 104
    $dirs = $optional + $(if ([BitConverter]::ToUInt16($b, $optional) -eq 0x20b) { 112 } else { 96 })
    $importRva = [BitConverter]::ToUInt32($b, $dirs + 8)
    $table = $optional + $optionalSize
    $toOffset = {
        param([uint32]$rva)
        for ($i = 0; $i -lt $sections; $i++) {
            $s = $table + 40 * $i
            $va = [BitConverter]::ToUInt32($b, $s + 12)
            $size = [Math]::Max([BitConverter]::ToUInt32($b, $s + 8), [BitConverter]::ToUInt32($b, $s + 16))
            if ($rva -ge $va -and $rva -lt $va + $size) { return $rva - $va + [BitConverter]::ToUInt32($b, $s + 20) }
        }
        throw "$File is not a Windows program this script can read"
    }
    if ($importRva -eq 0) { return @() }
    $names = @()
    for ($d = & $toOffset $importRva; [BitConverter]::ToUInt32($b, $d + 12) -ne 0; $d += 20) {
        $n = & $toOffset ([BitConverter]::ToUInt32($b, $d + 12))
        $end = [Array]::IndexOf($b, [byte]0, [int]$n)
        $names += [Text.Encoding]::ASCII.GetString($b, $n, $end - $n)
    }
    $names
}
# Our programs carry the C runtime (+crt-static above): one that needs
# the Visual C++ runtime would not start on a PC without it.
function Assert-NoVcRuntime([string]$File) {
    $needs = Get-DllImports $File | Where-Object { $_ -match '^(vcruntime|msvcp|concrt)\d' }
    if ($needs) { throw "$File needs $($needs -join ', '): it would not start on a Windows without the Visual C++ runtime." }
}

function Assert-NoLocalPaths([string]$File) {
    $bytes = [IO.File]::ReadAllBytes($File)
    # As bytes (C strings) and as UTF-16 (Windows and .NET strings, MSVC's assert()).
    $texts = @([Text.Encoding]::GetEncoding(28591).GetString($bytes), [Text.Encoding]::Unicode.GetString($bytes), [Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1))
    foreach ($local in @($env:USERPROFILE, $repoRoot)) {
        foreach ($form in @($local, $local.Replace("\", "/"))) {
            foreach ($text in $texts) {
                if ($text.IndexOf($form, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    throw "$File contains the local path $form."
                }
            }
        }
    }
}

Write-Host "`n== Workers ==" -ForegroundColor Cyan
Invoke-Native "building the workers" { node scripts/build-workers.mjs --release }
$workers = Join-Path $repoRoot "src-tauri\resources\workers"
Assert-NoLocalPaths (Join-Path $workers "worker-core.exe")
Assert-NoVcRuntime (Join-Path $workers "worker-core.exe")
Assert-NoLocalPaths (Join-Path $workers "office-worker.exe")

Write-Host "`n== App ==" -ForegroundColor Cyan
Invoke-Native "building the interface" { npm --prefix ui run build }
Invoke-Native "building the app" { npx tauri build --no-bundle }
$appExe = Join-Path $repoRoot "target\release\mqkit.exe"
Assert-NoLocalPaths $appExe
Assert-NoVcRuntime $appExe

if ($signing) {
    Write-Host "`n== Signing the programs ==" -ForegroundColor Cyan
    Read-Host "Plug in the signing token and press Enter" | Out-Null
    foreach ($p in @((Join-Path $workers "worker-core.exe"), (Join-Path $workers "office-worker.exe"), $appExe)) {
        & "$PSScriptRoot\sign-windows.ps1" -Path $p
    }
}

Write-Host "`n== Installer ==" -ForegroundColor Cyan
# Installers of earlier builds (and earlier names) stay in Tauri's bundle folder: cleared,
# so the one copied below can only be the one built now.
$bundleDir = Join-Path $repoRoot "target\release\bundle\nsis"
if (Test-Path $bundleDir) { Get-ChildItem $bundleDir -Filter "*-setup.exe" | Remove-Item -Force }
$productName = (Get-Content (Join-Path $repoRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).productName
if ($signing) {
    # Tauri signs what it generates while bundling: the uninstaller (inside the
    # installer) and the installer. An unsigned uninstall.exe that deletes itself
    # through %TEMP% is exactly what antivirus heuristics flag.
    # As a program and its arguments: Tauri runs it without a shell, so quotes in a single
    # command line would reach PowerShell as part of the path.
    $signScript = Join-Path $PSScriptRoot "sign-windows.ps1"
    $signConf = Join-Path $repoRoot "target\tauri.sign.conf.json"
    $signCommand = @{ cmd = "powershell"; args = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $signScript, "-Path", "%1") }
    $conf = @{ bundle = @{ windows = @{ signCommand = $signCommand } } }
    [IO.File]::WriteAllText($signConf, ($conf | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))
    Invoke-Native "bundling the installer" { npx tauri bundle --bundles nsis --config $signConf }
} else {
    Invoke-Native "bundling the installer" { npx tauri bundle --bundles nsis }
}
$built = Get-Item (Join-Path $bundleDir "${productName}_${version}_x64-setup.exe") -ErrorAction SilentlyContinue
if (-not $built) { throw "The NSIS installer ${productName}_${version}_x64-setup.exe was not found." }
New-Item -ItemType Directory -Force $dist | Out-Null
$setup = Join-Path $dist $(if ($Release) { "MQKit-setup-$version.exe" } elseif ($Sign) { "MQKit-setup-$version-signed-test.exe" } else { "MQKit-setup-$version-dev.exe" })
Copy-Item $built.FullName $setup -Force
Assert-NoLocalPaths $setup

if ($signing) {
    Write-Host "`n== Signatures ==" -ForegroundColor Cyan
    foreach ($path in @($setup, $appExe, (Join-Path $workers "worker-core.exe"), (Join-Path $workers "office-worker.exe"))) {
        $sig = Get-AuthenticodeSignature $path
        if ($sig.Status -ne "Valid") { throw "$(Split-Path -Leaf $path) is not validly signed: $($sig.Status)" }
        Write-Host ("  {0,-34} {1}" -f (Split-Path -Leaf $path), $sig.SignerCertificate.Subject.Split(",")[0])
    }
} else {
    Write-Host "Development build: not signed (use -Release for production)." -ForegroundColor Yellow
}

$hash = (Get-FileHash $setup -Algorithm SHA256).Hash
$size = [math]::Round((Get-Item $setup).Length / 1MB, 1)
Write-Host "`n$setup  ($size MB)" -ForegroundColor Green
Write-Host "SHA-256 $hash"

if ($Release) {
    # The feed of the update notification (src-tauri/src/update.rs: FEED_URL). Publish it
    # on the website only after the installer is downloadable.
    $feed = [ordered]@{
        version = $version
        url     = $ReleasePage
        notes   = [ordered]@{ ro = "Unelte noi și îmbunătățiri."; en = "New tools and improvements." }
    }
    [IO.File]::WriteAllText((Join-Path $dist "latest.json"), ($feed | ConvertTo-Json -Depth 4), (New-Object Text.UTF8Encoding $false))
    Write-Host "dist\latest.json written (edit the notes, then publish it at the feed address)."
    if ($InstallerUrl) {
        & "$PSScriptRoot\winget.ps1" -Version $version -Sha256 $hash -InstallerUrl $InstallerUrl
    } else {
        Write-Host "No -InstallerUrl: winget manifests not written." -ForegroundColor Yellow
    }
}
