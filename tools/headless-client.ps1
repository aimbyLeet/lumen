# A client of the headless stand: lumen-client to the service's host through the local server in
# bench-out\headless\svc, approved as the admin would. -KillAfterSec: cut it off (the session is lost, not closed).
#   tools\headless-client.ps1 -Name run1 -ClientArgs "--frames 300 --monitor-choice 2560x1440@100"
param([string]$Name, [string]$ClientArgs, [int]$KillAfterSec = 0, [int]$TimeoutSec = 90, [string]$Data = "client")
$rel = "C:\dev\lumen\target\release"; $d = "C:\dev\lumen\bench-out\headless\svc"
$token = ((& "$rel\lumen-host.exe" registration show) -match "^token: " | Select-Object -First 1) -replace "^token: ", ""
function Srv { & "$rel\lumen-server.exe" --db "$d\lumen.db" @args }
$env:LUMEN_DATA = "$d\$Data"
$c = Start-Process "$rel\lumen-client.exe" -PassThru `
    -ArgumentList "--server 127.0.0.1:7443 --server-ca `"$d\ca.pem`" --token $token --audio off --clipboard off $ClientArgs" `
    -RedirectStandardOutput "$d\$Name.log" -RedirectStandardError "$d\$Name.err"
Remove-Item Env:LUMEN_DATA
$t0 = Get-Date
while (-not $c.HasExited -and ((Get-Date) - $t0).TotalSeconds -lt $TimeoutSec) {
    $row = Srv access --status pending | Select-Object -First 1
    if ($row -match 'client\s+(\d+).*host\s+(\d+)') { Srv approve $Matches[1] $Matches[2] | Out-Null; "approved" }
    if ($KillAfterSec -gt 0 -and ((Get-Date) - $t0).TotalSeconds -ge $KillAfterSec) { Stop-Process -Id $c.Id -Force; "killed at $KillAfterSec s"; break }
    Start-Sleep -Milliseconds 500
}
if (-not $c.HasExited) { Stop-Process -Id $c.Id -Force; "timeout: killed" }
"exit $($c.ExitCode) after $([int]((Get-Date) - $t0).TotalSeconds) s"
