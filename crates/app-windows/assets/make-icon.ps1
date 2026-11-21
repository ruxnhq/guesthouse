# Makes guesthouse.ico, the icon build.rs puts in guesthouse-app.exe, from the app's icon
# (crates/app/data/icons/hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg). Windows has nothing
# that draws an SVG into an icon, so this draws the same shapes, at each size Windows asks for, and
# stores each one as a PNG. Change both together.
#
# The shapes are the SVG's, in a deeper palette that sits better with Windows 11's icons. At 32 px
# and below a plainer drawing takes over: thinner walls drown the little house at that size.
#   powershell -ExecutionPolicy Bypass -File crates\app-windows\assets\make-icon.ps1
param([string]$IcoPath = (Join-Path $PSScriptRoot 'guesthouse.ico'), [string]$PngFolder)
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

# A house: its roof's peak, where the roof meets the walls, and the floor.
function House($left, $right, $peak, $eaves, $floor, $dy = 0) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $mid = ($left + $right) / 2
    $path.AddPolygon([System.Drawing.PointF[]]@(
            (New-Object System.Drawing.PointF $mid, ($peak + $dy)),
            (New-Object System.Drawing.PointF $right, ($eaves + $dy)),
            (New-Object System.Drawing.PointF $right, ($floor + $dy)),
            (New-Object System.Drawing.PointF $left, ($floor + $dy)),
            (New-Object System.Drawing.PointF $left, ($eaves + $dy))))
    $path
}

function Edge($brush, $width) {
    $pen = New-Object System.Drawing.Pen $brush, $width
    $pen.LineJoin = 'Round'
    $pen
}

$wallTop = '#3aa0ff'; $wallBottom = '#0b5cc0'; $wallDark = '#084891'; $wallSmall = '#0a58b8'
$tint = '#e6f3ff'; $warmTop = '#ffe28f'; $warmBottom = '#ff9e2c'; $warmDeep = '#e8820c'

function Draw($n) {
    $bmp = New-Object System.Drawing.Bitmap $n, $n, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.CompositingQuality = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)

    if ($n -eq 16) {
        # Drawn to the pixel: two pixel walls centred on whole pixels, and the little house's
        # walls on pixel edges, so nothing lands half on one.
        $outer = House 2 14 2.5 7.5 14
        $g.FillPath((New-Object System.Drawing.SolidBrush (Color $tint)), $outer)
        $g.DrawPath((Edge (Color $wallSmall) 2), $outer)
        $g.FillPath((New-Object System.Drawing.SolidBrush (Color $warmBottom)), (House 5 11 6 9 12))
    } elseif ($n -le 32) {
        # The small drawing, on a 32 unit grid.
        $g.ScaleTransform($n / 32, $n / 32)
        $outer = House 5 27 4.5 14 26.5
        $g.FillPath((New-Object System.Drawing.SolidBrush (Color $tint)), $outer)
        $g.DrawPath((Edge (Color $wallSmall) 3.5), $outer)
        $inner = House 11 21 14 18 21.5
        $g.FillPath((New-Object System.Drawing.SolidBrush (Color $warmBottom)), $inner)
        $g.DrawPath((Edge (Color $warmBottom) 2.5), $inner)
    } else {
        # The SVG's own units from here on.
        $g.ScaleTransform($n / 128, $n / 128)
        $g.FillEllipse((New-Object System.Drawing.SolidBrush (Color '#000000' 56)), 18, 113, 92, 10)
        # The walls' underside, showing below them and under the roof inside.
        $g.DrawPath((Edge (Color $wallDark) 12), (House 22 106 22 58 106 4))
        $outer = House 22 106 22 58 106
        $g.FillPath((New-Object System.Drawing.SolidBrush (Color $tint)), $outer)
        $g.DrawPath((Edge (Downward 16 112 $wallTop $wallBottom) 12), $outer)
        # The little house's glow on the tint.
        $glow = New-Object System.Drawing.Drawing2D.GraphicsPath
        $glow.AddEllipse(32, 52, 64, 48)
        $light = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
        $light.CenterColor = Color $warmTop 140
        $light.SurroundColors = @(Color $warmTop 0)
        $g.FillPath($light, $glow)
        $inner = House 45 83 58 74 89
        $g.FillPath((Downward 53 93 $warmTop $warmBottom), $inner)
        $g.DrawPath((Edge (Downward 53 93 $warmTop $warmBottom) 8), $inner)
        $g.FillPath((New-Object System.Drawing.SolidBrush (Color $warmDeep)), (Rounded 60 80 8 13 2))
    }

    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $ms.ToArray()
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$pngs = foreach ($n in $sizes) { , (Draw $n) }
if ($PngFolder) {
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        [System.IO.File]::WriteAllBytes((Join-Path $PngFolder "$($sizes[$i]).png"), $pngs[$i])
    }
}

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
[System.IO.File]::WriteAllBytes($IcoPath, $out.ToArray())
"guesthouse.ico: $($sizes -join ', ') px, $($out.Length) bytes"
