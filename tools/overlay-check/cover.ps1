# A plain window of the check's own, to put over the video window. Runs until the check kills it.
param([int]$X, [int]$Y, [int]$W, [int]$H)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c); }
"@
[void][Dpi]::SetProcessDpiAwarenessContext([IntPtr](-4))
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$f = New-Object System.Windows.Forms.Form
$f.Text = "Lumen Gate Cover"
$f.FormBorderStyle = "None"
$f.ShowInTaskbar = $false
$f.StartPosition = "Manual"
$f.Bounds = New-Object System.Drawing.Rectangle $X, $Y, $W, $H
$f.BackColor = [System.Drawing.Color]::FromArgb(255, 0, 255)
[System.Windows.Forms.Application]::Run($f)
