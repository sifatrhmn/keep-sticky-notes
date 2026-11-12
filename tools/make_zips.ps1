# Packs the already-built portable apps in dist_x64 and dist_arm64 into ZIP files,
# without rebuilding, so each ZIP holds exactly the released executable.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$notice = @"
If Windows warns you
====================

This app is free and open source, but its files are not signed with a paid
code-signing certificate. Microsoft SmartScreen may therefore warn about it.

* Edge says the download "isn't commonly downloaded": choose ... > Keep.
* Windows says "Windows protected your PC": choose More info > Run anyway.
* Or, before running: right-click the .exe, choose Properties, tick Unblock, OK.

Source code and checksums: https://github.com/sifatrhmn/keep-sticky-notes
"@

foreach ($arch in 'x64', 'arm64') {
    $dist = Join-Path $root "dist_$arch"
    $exe = Join-Path $dist "KeepStickyNotes-$arch-portable.exe"
    if (-not (Test-Path $exe)) { throw "Missing $exe; run build_dist.ps1 first" }
    $stage = Join-Path $root "target\zip\KeepStickyNotes-$arch"
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item $exe $stage
    Copy-Item (Join-Path $root 'LICENSE') (Join-Path $stage 'LICENSE.txt')
    if (Test-Path (Join-Path $root 'README.md')) { Copy-Item (Join-Path $root 'README.md') $stage }
    Set-Content -Path (Join-Path $stage 'If Windows warns you.txt') -Value $notice -Encoding UTF8
    $zip = Join-Path $dist "KeepStickyNotes-$arch.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
    "{0}: {1} ({2:N1} MB)" -f $arch, $zip, ((Get-Item $zip).Length / 1MB)
}
