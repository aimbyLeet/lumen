# The app icon from icons\main_icon.png (square, a round picture on black):
# outside its circle made transparent, at every size Windows asks for, as
# icons\lumen.ico (16..256 px) and the PNGs the apps take:
#   apps\host-app\icons, apps\client-app\icons  (Tauri: the exe, its window,
#       the taskbar, shortcuts, the tray)
#   apps\client\lumen.ico                       (the video window's exe)
#   web\admin\public\favicon.png                (the admin's browser tab)
#   web\client\src\assets\lumen.png             (the hub's title bar)
#   icons\wizard-small.png                      (the setup pages' top right)
# The installer takes icons\lumen.ico. Run after changing main_icon.png.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Add-Type -AssemblyName System.Drawing

$src = [System.Drawing.Image]::FromFile("$root\icons\main_icon.png")
$n = $src.Width
$argb = [System.Drawing.Imaging.PixelFormat]::Format32bppArgb

# The round master: the picture inside its circle, with an anti-aliased edge.
$master = New-Object System.Drawing.Bitmap $n, $n, $argb
$g = [System.Drawing.Graphics]::FromImage($master)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)
$inset = [int]($n * 0.004)
$g.FillEllipse((New-Object System.Drawing.TextureBrush $src), $inset, $inset, $n - 2 * $inset, $n - 2 * $inset)
$g.Dispose()

function Scaled([int]$s) {
    $b = New-Object System.Drawing.Bitmap $s, $s, $argb
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($master, 0, 0, $s, $s)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $b.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $b.Dispose()
    , $ms.ToArray()
}

function Save-Png([int]$s, [string]$path) {
    [System.IO.File]::WriteAllBytes($path, (Scaled $s))
}

# An .ico of PNG entries (as Windows reads them since Vista).
function Save-Ico([string]$path) {
    $sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
    $pngs = $sizes | ForEach-Object { , (Scaled $_) }
    $ms = New-Object System.IO.MemoryStream
    $w = New-Object System.IO.BinaryWriter $ms
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $s = $sizes[$i] % 256
        $w.Write([byte]$s); $w.Write([byte]$s); $w.Write([byte]0); $w.Write([byte]0)
        $w.Write([uint16]1); $w.Write([uint16]32)
        $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
        $offset += $pngs[$i].Length
    }
    foreach ($p in $pngs) { $w.Write($p) }
    $w.Flush()
    [System.IO.File]::WriteAllBytes($path, $ms.ToArray())
}

Save-Ico "$root\icons\lumen.ico"
foreach ($app in 'host-app', 'client-app') {
    $d = "$root\apps\$app\icons"
    Copy-Item "$root\icons\lumen.ico" "$d\icon.ico" -Force
    Save-Png 512 "$d\icon.png"
    Save-Png 128 "$d\128x128.png"
    Save-Png 32 "$d\32x32.png"
    $master.Save("$d\source.png", [System.Drawing.Imaging.ImageFormat]::Png)
}
Copy-Item "$root\icons\lumen.ico" "$root\apps\client\lumen.ico" -Force
New-Item -ItemType Directory -Force "$root\web\admin\public" | Out-Null
Save-Png 64 "$root\web\admin\public\favicon.png"
Save-Png 128 "$root\web\client\src\assets\lumen.png"

# The setup's picture at the top right of its pages (WizardSmallImageFile): Inno stretches it over the whole
# area, so the icon sits in transparent margins — about as big as the uninstaller shows it (2026-11-09).
$canvas = 128
$icon = 62
$b = New-Object System.Drawing.Bitmap $canvas, $canvas, $argb
$g = [System.Drawing.Graphics]::FromImage($b)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)
$at = ($canvas - $icon) / 2
$g.DrawImage($master, $at, $at, $icon, $icon)
$g.Dispose()
$b.Save("$root\icons\wizard-small.png", [System.Drawing.Imaging.ImageFormat]::Png)
$b.Dispose()
"icons written from $root\icons\main_icon.png"
