# Draws the program's icon with GDI+ and packs it as assets/icon.ico (PNG frames). Run once by hand.
param([string]$Out = "$PSScriptRoot\..\assets\icon.ico")
Add-Type -AssemblyName System.Drawing

function New-Frame([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $s = $size / 256.0
    # a red disc, darker at the bottom
    $rect = New-Object System.Drawing.RectangleF (6 * $s), (6 * $s), (244 * $s), (244 * $s)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, ([System.Drawing.Color]::FromArgb(255, 255, 70, 66)), ([System.Drawing.Color]::FromArgb(255, 176, 16, 24)), 90.0
    $g.FillEllipse($brush, $rect)
    # the sound of a siren: two arcs either side of a white core
    $white = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), (16 * $s)
    $white.StartCap = 'Round'
    $white.EndCap = 'Round'
    foreach ($r in 62, 92) {
        $d = $r * 2 * $s
        $c = 128 * $s
        $box = New-Object System.Drawing.RectangleF ($c - $d / 2), ($c - $d / 2), $d, $d
        $g.DrawArc($white, $box, -40, 80)
        $g.DrawArc($white, $box, 140, 80)
    }
    $core = 58 * $s
    $g.FillEllipse([System.Drawing.Brushes]::White, (128 * $s - $core / 2), (128 * $s - $core / 2), $core, $core)
    $g.Dispose()
    return $bmp
}

$sizes = 256, 64, 48, 32, 24, 16
$frames = foreach ($n in $sizes) {
    $bmp = New-Frame $n
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $ms.ToArray()
}
$pack = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $pack
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $n = $sizes[$i]
    $w.Write([byte]($(if ($n -ge 256) { 0 } else { $n })))
    $w.Write([byte]($(if ($n -ge 256) { 0 } else { $n })))
    $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$frames[$i].Length)
    $w.Write([uint32]$offset)
    $offset += $frames[$i].Length
}
foreach ($f in $frames) { $w.Write([byte[]]$f) }
[System.IO.File]::WriteAllBytes($Out, $pack.ToArray())
"icon: $((Get-Item $Out).Length) bytes -> $Out"
