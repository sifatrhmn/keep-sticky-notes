# Builds, verifies and packages both architectures into dist_x64 and dist_arm64.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
Add-Type -AssemblyName System.Drawing

function Get-PeMachine([string]$path) {
    $bytes = [IO.File]::ReadAllBytes($path)
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    return [BitConverter]::ToUInt16($bytes, $pe + 4)
}

function Save-Logo([string]$out, [int]$w, [int]$h) {
    $src = [Drawing.Image]::FromFile("$root\res\logo.png")
    $bmp = New-Object Drawing.Bitmap $w, $h
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = 'HighQualityBicubic'
    $side = [Math]::Min($w, $h)
    $g.DrawImage($src, [int](($w - $side) / 2), [int](($h - $side) / 2), $side, $side)
    $g.Dispose(); $src.Dispose()
    $bmp.Save($out, [Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

$assets = "$root\packaging\Assets"
New-Item -ItemType Directory -Force $assets | Out-Null
Save-Logo "$assets\StoreLogo.png" 50 50
Save-Logo "$assets\Square44x44Logo.png" 44 44
Save-Logo "$assets\Square150x150Logo.png" 150 150
Save-Logo "$assets\Wide310x150Logo.png" 310 150

# The private key stays in packaging\; only the public certificate is distributed.
$manifestBase = Get-Content "$root\packaging\Package.appxmanifest" -Raw
$pfx = "$root\packaging\devcert.pfx"
$passFile = "$root\packaging\devcert.password.txt"
if (-not (Test-Path $passFile)) { [Guid]::NewGuid().ToString('N') | Set-Content -NoNewline $passFile }
$pass = (Get-Content $passFile -Raw).Trim()
if (-not (Test-Path $pfx)) {
    & winapp cert generate --publisher "CN=Keep Sticky Notes Local Build" --output $pfx --password $pass
    if ($LASTEXITCODE -ne 0) { throw 'certificate generation failed' }
}
$cert = New-Object Security.Cryptography.X509Certificates.X509Certificate2 $pfx, $pass
$cerBytes = $cert.Export('Cert')

$targets = @(
    @{ Triple = 'x86_64-pc-windows-msvc';  Arch = 'x64';   Dist = 'dist_x64';   Machine = 0x8664 },
    @{ Triple = 'aarch64-pc-windows-msvc'; Arch = 'arm64'; Dist = 'dist_arm64'; Machine = 0xAA64 }
)
foreach ($t in $targets) {
    & cargo build --release --target $t.Triple
    if ($LASTEXITCODE -ne 0) { throw "build failed for $($t.Triple)" }
    $exe = "$root\target\$($t.Triple)\release\KeepStickyNotes.exe"
    $machine = Get-PeMachine $exe
    if ($machine -ne $t.Machine) { throw ("{0}: wrong PE machine 0x{1:X4}" -f $t.Arch, $machine) }

    $dist = "$root\$($t.Dist)"
    New-Item -ItemType Directory -Force $dist | Out-Null
    Get-ChildItem $dist -File | Remove-Item -Force
    Copy-Item $exe "$dist\KeepStickyNotes-$($t.Arch)-portable.exe"
    Copy-Item "$root\LICENSE" "$dist\LICENSE.txt"

    $stage = "$root\target\stage\$($t.Arch)"
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item $exe $stage
    Copy-Item -Recurse $assets "$stage\Assets"
    $manifest = "$root\target\stage\Package.$($t.Arch).appxmanifest"
    [IO.File]::WriteAllText($manifest, $manifestBase.Replace('ARCH_PLACEHOLDER', $t.Arch))
    $msix = "$dist\KeepStickyNotes-$($t.Arch).msix"
    & winapp package $stage --manifest $manifest --cert $pfx --cert-password $pass --output $msix --skip-pri
    if ($LASTEXITCODE -ne 0) { throw "packaging failed for $($t.Arch)" }
    [IO.File]::WriteAllBytes("$dist\KeepStickyNotes-signing-certificate.cer", $cerBytes)

    $check = "$root\target\stage\check-$($t.Arch)"
    Remove-Item -Recurse -Force $check -ErrorAction SilentlyContinue
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($msix, $check)
    Get-ChildItem $check -Recurse -Include *.exe, *.dll | ForEach-Object {
        $m = Get-PeMachine $_.FullName
        if ($m -ne $t.Machine) { throw ("{0} has machine 0x{1:X4}" -f $_.Name, $m) }
        "{0}: {1} in MSIX is 0x{2:X4}" -f $t.Arch, $_.Name, $m
    }
    "{0}: portable PE machine 0x{1:X4}, {2:N1} MB" -f $t.Arch, $machine, ((Get-Item $exe).Length / 1MB)
}
