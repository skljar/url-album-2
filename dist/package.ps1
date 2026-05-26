# Упаковка URL Album — один 32-битный exe, работает на Win7 SP1+ (x86 и x64).
$version = "2.0.2"
$root    = "C:\Projects\url-album-3"
$srcExe  = "$root\target\i686-pc-windows-msvc\release\url-album-3.exe"
$destDir = "$root\dist\URL-Album-$version"
$zipPath = "$root\dist\URL-Album-$version.zip"

if (-not (Test-Path $srcExe)) {
    Write-Error "exe not found: $srcExe"
    Write-Host "Run: cargo build --release && pe-patch"
    exit 1
}

New-Item -ItemType Directory -Force "$destDir\Data\favicons" | Out-Null
Copy-Item $srcExe "$destDir\URL-Album-$version.exe" -Force

@"
URL Album $version - Portable Bookmark Manager
===============================================
Requirements:
  Windows 7 SP1 / 8 / 10 / 11 (32-bit and 64-bit)
  No additional runtimes required (CRT is statically linked).

Run: URL-Album-$version.exe
Data stored next to exe:
  album.db        - bookmark database
  settings.json   - settings
  recent_dbs.txt  - recent databases list
  Data\favicons\  - favicon cache

Fully portable - nothing written to registry.
"@ | Out-File "$destDir\README.txt" -Encoding utf8

if (Test-Path $zipPath) { Remove-Item $zipPath }
Compress-Archive -Path "$destDir\*" -DestinationPath $zipPath
$size = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)
Write-Host "Done: $zipPath ($size MB)"
