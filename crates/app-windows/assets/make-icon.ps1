# Makes guesthouse.ico, the icon build.rs puts in guesthouse-app.exe, from the app's icon
# (crates/app/data/icons/hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg). Windows has nothing
# that draws an SVG into an icon, so this draws the same shapes, at each size Windows asks for, and
# stores each one as a PNG. Change both together.
#   powershell -ExecutionPolicy Bypass -File crates\app-windows\assets\make-icon.ps1
Add-Type -AssemblyName System.Drawing

function Color($hex, $alpha = 255) {
    [System.Drawing.Color]::FromArgb($alpha, [Convert]::ToInt32($hex.Substring(1, 2), 16),
        [Convert]::ToInt32($hex.Substring(3, 2), 16), [Convert]::ToInt32($hex.Substring(5, 2), 16))
}

function Rounded($x, $y, $w, $h, $r) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc($x, $y, 2 * $r, 2 * $r, 180, 90)
    $path.AddArc($x + $w - 2 * $r, $y, 2 * $r, 2 * $r, 270, 90)
    $path.AddArc($x + $w - 2 * $r, $y + $h - 2 * $r, 2 * $r, 2 * $r, 0, 90)
    $path.AddArc($x, $y + $h - 2 * $r, 2 * $r, 2 * $r, 90, 90)
    $path.CloseFigure()
    $path
}

# A gradient from top to bottom, as the SVG's linear gradients all are.
function Downward($top, $bottom, $from, $to) {
    New-Object System.Drawing.Drawing2D.LinearGradientBrush (New-Object System.Drawing.PointF 0, $top),
        (New-Object System.Drawing.PointF 0, $bottom), (Color $from), (Color $to)
}

function Draw($n) {
    $bmp = New-Object System.Drawing.Bitmap $n, $n, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.CompositingQuality = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    # The SVG's own units from here on.
    $g.ScaleTransform($n / 128, $n / 128)

    $g.FillPath((New-Object System.Drawing.SolidBrush (Color '#241f31')), (Rounded 84 22 14 30 2))
    $g.FillPath((New-Object System.Drawing.SolidBrush (Color '#1a5fb4')), (Rounded 24 52 80 64 6))
    $wall = Rounded 24 48 80 64 6
    $g.FillPath((Downward 50 112 '#3584e4' '#1c71d8'), $wall)
    # The window's glow on the wall.
    $glow = New-Object System.Drawing.Drawing2D.GraphicsPath
    $glow.AddEllipse(30, 48, 68, 68)
    $light = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
    $light.CenterColor = Color '#f9f06b' 140
    $light.SurroundColors = @(Color '#f9f06b' 0)
    $g.SetClip($wall)
    $g.FillPath($light, $glow)
    $g.ResetClip()

    $roof = New-Object System.Drawing.Drawing2D.GraphicsPath
    $roof.AddPolygon([System.Drawing.PointF[]]@(
            (New-Object System.Drawing.PointF 64, 16),
            (New-Object System.Drawing.PointF 112, 56),
            (New-Object System.Drawing.PointF 16, 56)))
    $g.FillPath((Downward 14 64 '#5e5c64' '#3d3846'), $roof)
    $edge = New-Object System.Drawing.Pen (Downward 14 64 '#5e5c64' '#3d3846'), 10
    $edge.LineJoin = 'Round'
    $g.DrawPath($edge, $roof)

    $eave = New-Object System.Drawing.Pen (Color '#241f31'), 4
    $eave.StartCap = 'Round'
    $eave.EndCap = 'Round'
    $g.DrawLine($eave, 11, 61, 117, 61)

    $g.FillPath((New-Object System.Drawing.SolidBrush (Color '#1a5fb4')), (Rounded 46 68 36 32 3))
    $g.FillPath((Downward 66 98 '#f9f06b' '#f6d32d'), (Rounded 48 70 32 28 2))
    $bars = New-Object System.Drawing.Pen (Color '#1a5fb4'), 3
    $g.DrawLine($bars, 64, 70, 64, 98)
    $g.DrawLine($bars, 48, 84, 80, 84)

    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $ms.ToArray()
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$pngs = foreach ($n in $sizes) { , (Draw $n) }

# An ICONDIR, an ICONDIRENTRY for each size, then the PNGs, which Windows has taken since Vista.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    # 256 is written as 0.
    $n = $sizes[$i] % 256
    $len = $pngs[$i].Length
    $w.Write([byte]$n); $w.Write([byte]$n); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$len); $w.Write([uint32]$offset)
    $offset += $len
}
foreach ($p in $pngs) { $w.Write($p) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot 'guesthouse.ico'), $out.ToArray())
"guesthouse.ico: $($sizes -join ', ') px, $($out.Length) bytes"
