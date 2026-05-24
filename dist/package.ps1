# Упаковка URL Album 3 для x64 и x86
$root = "C:\Projects\url-album-3"

foreach ($arch in @("x86_64-pc-windows-msvc", "i686-pc-windows-msvc")) {
    $shortArch = if ($arch -like "*x86_64*") { "x64" } else { "x86" }
    $srcExe    = "$root\target\$arch\release\url-album-3.exe"
    $destDir   = "$root\dist\URL-Album-3-$shortArch"

    if (-not (Test-Path $srcExe)) {
        Write-Host "[$shortArch] exe not found: $srcExe -- skipping"
        continue
    }

    # Копируем exe
    Copy-Item $srcExe "$destDir\URL-Album.exe" -Force

    # Создаём пустые portable-файлы/папки
    New-Item -ItemType Directory -Force "$destDir\Data\favicons" | Out-Null

    # README
    $winNote = if ($shortArch -eq "x86") {
        "Windows 7 SP1 / 8 / 10 / 11 (32-bit and 64-bit)"
    } else {
        "Windows 7 SP1 / 8 / 10 / 11 (64-bit)"
    }
    $readmeText = @"
URL Album 3 - Portable Bookmark Manager
========================================
Version: 3.0 ($shortArch)

Requirements:
  $winNote
  No additional runtimes required (CRT is statically linked).
  Windows 7 only: also requires Platform Update (KB2670838)

Run: URL-Album.exe
Data stored next to exe:
  album.db        - bookmark database
  settings.json   - settings
  recent_dbs.txt  - recent databases list
  Data\favicons\  - favicon cache

Fully portable - nothing written to registry.
"@
    $readmeText | Out-File "$destDir\README.txt" -Encoding utf8

    # ZIP
    $zipPath = "$root\dist\URL-Album-3-$shortArch.zip"
    if (Test-Path $zipPath) { Remove-Item $zipPath }
    Compress-Archive -Path "$destDir\*" -DestinationPath $zipPath
    $size = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)
    Write-Host "[$shortArch] Done: $zipPath ($size MB)"
}
