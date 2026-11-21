# Development helper for the "Lumen Host" service (run once, as administrator).
#
# While the service runs it holds target\release\lumen-host.exe, which then
# cannot be rebuilt, and only an administrator may stop it. This script stops
# and starts that one service when asked through flag files, so a rebuild needs
# no elevated prompt each time:
#   bench-out\svc\stop.flag   -> Stop-Service LumenHost  (flag removed when done)
#   bench-out\svc\start.flag  -> Start-Service LumenHost (flag removed when done)
# It does nothing else. Close the window (or Ctrl+C) to end it.
$ErrorActionPreference = "Continue"
$dir = Join-Path (Split-Path -Parent $PSScriptRoot) "bench-out\svc"
New-Item -ItemType Directory -Force $dir | Out-Null
Write-Host "Watching $dir for stop.flag / start.flag (service LumenHost). Ctrl+C to end."
while ($true) {
    foreach ($op in "stop", "start") {
        $flag = Join-Path $dir "$op.flag"
        if (Test-Path $flag) {
            if ($op -eq "stop") { Stop-Service LumenHost -Force } else { Start-Service LumenHost }
            Write-Host "$(Get-Date -Format HH:mm:ss) $op -> $((Get-Service LumenHost).Status)"
            Remove-Item $flag -Force
        }
    }
    Start-Sleep -Milliseconds 300
}
