# What the checks ask of Windows, a line of JSON per call.
param([string]$Cmd, [string]$A = "", [string]$B = "", [string]$C = "", [string]$D = "")
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
  public struct R { public int L, T, Rt, B; }
  public struct P { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(P p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint f);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out P p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint c);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
  public static List<IntPtr> Of(uint pid) {
    var o = new List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid) o.Add(h); return true; }, IntPtr.Zero);
    return o;
  }
  public static string Text(IntPtr h) { var s = new StringBuilder(256); GetWindowText(h, s, 256); return s.ToString(); }
  public static string Class(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
}
"@
[void][W]::SetProcessDpiAwarenessContext([IntPtr](-4))
$out = switch ($Cmd) {
  "windows" {
    # A process's visible and hidden top-level windows with a title.
    [W]::Of([uint32]$A) | Where-Object { [W]::Text($_) -ne "" } | ForEach-Object {
      $r = New-Object W+R; [void][W]::GetWindowRect($_, [ref]$r)
      @{ hwnd = $_.ToInt64(); title = [W]::Text($_); cls = [W]::Class($_); visible = [W]::IsWindowVisible($_); rect = @($r.L, $r.T, $r.Rt, $r.B) }
    }
  }
  "state" {
    # About a window: visible, rect, styles; the video window's client area on the screen.
    $h = [IntPtr][int64]$A
    $r = New-Object W+R; [void][W]::GetWindowRect($h, [ref]$r)
    $cl = New-Object W+R; [void][W]::GetClientRect($h, [ref]$cl)
    $p = New-Object W+P; [void][W]::ClientToScreen($h, [ref]$p)
    @{ visible = [W]::IsWindowVisible($h); rect = @($r.L, $r.T, $r.Rt, $r.B); client = @($p.X, $p.Y, ($p.X + $cl.Rt), ($p.Y + $cl.B)); ex = [W]::GetWindowLong($h, -20); fg = [W]::GetForegroundWindow().ToInt64(); prev = [W]::GetWindow($h, 3).ToInt64() }
  }
  "post" { @{ ok = [W]::PostMessage([IntPtr][int64]$A, [uint32]$B, [IntPtr][int64]$C, [IntPtr]::Zero) } }
  "front" { @{ ok = [W]::SetForegroundWindow([IntPtr][int64]$A) } }
  "frompoint" {
    $p = New-Object W+P; $p.X = [int]$A; $p.Y = [int]$B
    $h = [W]::WindowFromPoint($p)
    @{ hwnd = [W]::GetAncestor($h, 2).ToInt64(); cls = [W]::Class($h) }
  }
  "click" {
    # $C: the video window. Nothing happens unless it is in front and the point is on it: real input goes wherever the point is.
    $video = [int64]$C
    $before = [W]::GetAncestor([W]::WindowFromPoint((New-Object W+P -Property @{X=[int]$A;Y=[int]$B})), 2).ToInt64()
    if ([W]::GetForegroundWindow().ToInt64() -ne $video -or $before -ne $video) { @{ refused = $true; fg = [W]::GetForegroundWindow().ToInt64(); under = $before }; break }
    $o = New-Object W+P; [void][W]::GetCursorPos([ref]$o)
    [void][W]::SetCursorPos([int]$A, [int]$B)
    Start-Sleep -Milliseconds 150
    $h = [W]::WindowFromPoint((New-Object W+P -Property @{X=[int]$A;Y=[int]$B}))
    $under = [W]::GetAncestor($h, 2).ToInt64()
    [W]::mouse_event(2, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 40; [W]::mouse_event(4, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 300
    $fg = [W]::GetForegroundWindow().ToInt64()
    [void][W]::SetCursorPos($o.X, $o.Y)
    @{ under = $under; fg = $fg }
  }
  "above" {
    # Is window $A above window $B in the z-order?
    $target = [IntPtr][int64]$A
    $h = [IntPtr][int64]$B
    $found = $false
    while ($true) {
      $h = [W]::GetWindow($h, 3)
      if ($h -eq [IntPtr]::Zero) { break }
      if ($h -eq $target) { $found = $true; break }
    }
    @{ above = $found }
  }
  "clickon" {
    # A real click at a point only if window $C is what is under it and the foreground window is $D or $C: the check's own.
    $want = [int64]$C
    $own = @($want, [int64]$D)
    $pt = New-Object W+P -Property @{X=[int]$A;Y=[int]$B}
    $before = [W]::GetAncestor([W]::WindowFromPoint($pt), 2).ToInt64()
    $fg0 = [W]::GetForegroundWindow().ToInt64()
    if ($before -ne $want -or $own -notcontains $fg0) { @{ refused = $true; under = $before; fg = $fg0 }; break }
    $o = New-Object W+P; [void][W]::GetCursorPos([ref]$o)
    [void][W]::SetCursorPos([int]$A, [int]$B)
    Start-Sleep -Milliseconds 100
    [W]::mouse_event(2, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 40; [W]::mouse_event(4, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 300
    $fg = [W]::GetForegroundWindow().ToInt64()
    [void][W]::SetCursorPos($o.X, $o.Y)
    @{ under = $before; fg = $fg }
  }
  "shot" {
    Add-Type -AssemblyName System.Drawing
    $x = [int]$B.Split(",")[0]; $y = [int]$B.Split(",")[1]; $w = [int]$B.Split(",")[2]; $hh = [int]$B.Split(",")[3]
    $bmp = New-Object System.Drawing.Bitmap $w, $hh
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $bmp.Save($A, [System.Drawing.Imaging.ImageFormat]::Png)
    @{ ok = $true }
  }
  "cpu" {
    # Processes whose command line has $A: CPU seconds and memory, MB.
    Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($A) } | ForEach-Object {
      $p = Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue
      if ($p) { @{ pid = $p.Id; name = $p.ProcessName; cpu = $p.CPU; mb = [math]::Round($p.WorkingSet64 / 1MB) } }
    }
  }
}
ConvertTo-Json -Compress -Depth 4 -InputObject @($out)
