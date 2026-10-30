# Builds the portable Windows zip into target\release\package\ from a release build:
#   powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
# Only target\ is written. The executable is not code-signed.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT') { throw 'package-windows.ps1 runs on Windows only' }

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = ((cargo pkgid -p profile-studio) -split '[@#]')[-1]
if ($LASTEXITCODE -ne 0) { throw 'cargo pkgid failed' }
$hostTriple = (rustc -vV | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
$arch = switch -Wildcard ($hostTriple) { 'x86_64-*' { 'x64' } 'aarch64-*' { 'arm64' } default { $hostTriple } }

cargo build --release --locked -p profile-studio
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$name = "orca-profile-studio-$version-windows-$arch"
$packages = Join-Path $root 'target\release\package'
$dir = Join-Path $packages $name
$zip = "$dir.zip"
foreach ($old in @($dir, $zip)) { if (Test-Path $old) { Remove-Item -Recurse -Force $old } }
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item (Join-Path $root 'target\release\profile-studio.exe') (Join-Path $dir 'Orca Profile Studio.exe')
Copy-Item (Join-Path $root 'crates\profile-studio\resources\windows\README.txt') (Join-Path $dir 'README.txt')
Compress-Archive -Path $dir -DestinationPath $zip

$exe = Get-Item (Join-Path $dir 'Orca Profile Studio.exe')
if ($exe.VersionInfo.ProductVersion -ne $version) {
    throw "the executable reports version '$($exe.VersionInfo.ProductVersion)', expected '$version'"
}
Write-Output "built $zip ($version, $arch)"
