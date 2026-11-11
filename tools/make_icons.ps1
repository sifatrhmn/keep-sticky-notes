# Generates the original application icon (res/app.ico) and a master PNG (res/logo.png).
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot

function New-LogoPng([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $m = [single]($size * 0.08); $s = [single]($size - 2 * $m); $fold = [single]($s * 0.30)
    $body = [System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF $m, $m),
        (New-Object System.Drawing.PointF ($m + $s), $m),
        (New-Object System.Drawing.PointF ($m + $s), ($m + $s - $fold)),
        (New-Object System.Drawing.PointF ($m + $s - $fold), ($m + $s)),
        (New-Object System.Drawing.PointF $m, ($m + $s)))
    $g.FillPolygon((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 251, 188, 4))), $body)
    $flap = [System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF ($m + $s), ($m + $s - $fold)),
        (New-Object System.Drawing.PointF ($m + $s - $fold), ($m + $s - $fold)),
        (New-Object System.Drawing.PointF ($m + $s - $fold), ($m + $s)))
    $g.FillPolygon((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 200, 140, 0))), $flap)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 90, 70, 10)), ([single][Math]::Max(1, $size * 0.06))
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'
    foreach ($k in 0.30, 0.48) {
        $y = [single]($m + $s * $k)
        $g.DrawLine($pen, [single]($m + $s * 0.18), $y, [single]($m + $s * 0.82), $y)
    }
    $g.DrawLine($pen, [single]($m + $s * 0.18), [single]($m + $s * 0.66), [single]($m + $s * 0.5), [single]($m + $s * 0.66))
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return , $ms.ToArray()
}

[IO.Directory]::CreateDirectory("$root\res") | Out-Null
$sizes = 16, 24, 32, 48, 256
$images = @{}
foreach ($s in $sizes) { $images[$s] = New-LogoPng $s }
[IO.File]::WriteAllBytes("$root\res\logo.png", (New-LogoPng 512))

$fs = [IO.File]::Create("$root\res\app.ico")
$w = New-Object IO.BinaryWriter $fs
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($s in $sizes) {
    $dim = if ($s -ge 256) { 0 } else { $s }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$images[$s].Length); $w.Write([uint32]$offset)
    $offset += $images[$s].Length
}
foreach ($s in $sizes) { $w.Write([byte[]]$images[$s]) }
$w.Close()
