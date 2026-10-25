# Builds a release of ytmusic-lite for Windows.
#
#   powershell -ExecutionPolicy Bypass -File package\build-package.ps1
#
# It builds the program, gathers everything the app needs into dist\ytmusic-lite-<version>-win64\,
# makes a zip of that folder, and (if Inno Setup is installed) also an installer.
# At the end it prints the SHA-256 and the lines to put in the relay's latest.json.
#
# -SkipBuild  reuse the existing target\release\ytmusic-lite.exe

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$version = (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw "Couldn't read the version from Cargo.toml" }

if (-not $SkipBuild) {
    cargo test --release
    if ($LASTEXITCODE -ne 0) { throw "The tests failed, so no release was made." }
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "The build failed." }
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
$exe = Join-Path $targetDir "release\ytmusic-lite.exe"
if (-not (Test-Path $exe)) { throw "Missing $exe. Build first." }

# the tools the app starts, next to the program (it looks for them in its own folder)
$tools = "libmpv-2.dll", "yt-dlp.exe", "deno.exe"
foreach ($t in $tools) {
    if (-not (Test-Path (Join-Path $root "vendor\$t"))) { throw "Missing vendor\$t" }
}

$name = "ytmusic-lite-$version-win64"
$dist = Join-Path $root "dist"
$stage = Join-Path $dist $name
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item $stage -ItemType Directory -Force | Out-Null

Copy-Item $exe $stage
foreach ($t in $tools) { Copy-Item (Join-Path $root "vendor\$t") $stage }
Copy-Item (Join-Path $PSScriptRoot "README.txt") $stage
Copy-Item (Join-Path $PSScriptRoot "THIRD-PARTY-NOTICES.txt") $stage

$zip = Join-Path $dist "$name.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [System.IO.Compression.CompressionLevel]::Optimal, $true)

$files = @($zip)
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($iscc) {
    & $iscc "/DAppVersion=$version" "/DStageDir=$stage" "/O$dist" (Join-Path $PSScriptRoot "installer.iss")
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed." }
    $files += (Join-Path $dist "ytmusic-lite-setup-$version.exe")
} else {
    Write-Host "Inno Setup isn't installed, so only the zip was made (see package\RELEASING.md)."
}

Write-Host ""
Write-Host "Built version $version"
foreach ($f in $files) {
    $hash = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
    Write-Host ("  {0}  ({1:N0} MB)" -f $f, ((Get-Item $f).Length / 1MB))
    Write-Host "    sha256 $hash"
}
Write-Host ""
Write-Host "Once the file is uploaded, publish it by editing latest.json on the relay:"
Write-Host "  { `"version`": `"$version`", `"url`": `"<page or file people download from>`", `"notes`": `"<what changed>`" }"
