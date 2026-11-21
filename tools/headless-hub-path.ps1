# The hub's path without the hub: lumen-client as Lumen Client runs it (--status-json, the
# token on stdin), the answer to a host without monitors as the hub's command line on stdin. A new -Data folder is a new
# client device (no set at the service yet: the host asks).
#   tools\headless-hub-path.ps1 -Data client3
param([string]$Name = "hub", [string]$Data = "client2", [string]$Answer = '{"cmd":"monitor_choice","width":1920,"height":1080,"scale":100}', [int]$Frames = 300)
$rel = "C:\dev\lumen\target\release"; $d = "C:\dev\lumen\bench-out\headless\svc"
$token = ((& "$rel\lumen-host.exe" registration show) -match "^token: " | Select-Object -First 1) -replace "^token: ", ""
function Srv { & "$rel\lumen-server.exe" --db "$d\lumen.db" @args }
New-Item -ItemType Directory -Force "$d\$Data" | Out-Null
$psi = New-Object System.Diagnostics.ProcessStartInfo "$rel\lumen-client.exe"
$psi.Arguments = "--server 127.0.0.1:7443 --server-ca `"$d\ca.pem`" --token-stdin --status-json --audio off --clipboard off --frames $Frames"
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables["LUMEN_DATA"] = "$d\$Data"
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding $false
$p = [System.Diagnostics.Process]::Start($psi)
$p.StandardInput.WriteLine($token)
$p.StandardInput.Flush()
$lines = New-Object System.Collections.Concurrent.ConcurrentQueue[string]
$job = Register-ObjectEvent -InputObject $p -EventName OutputDataReceived -Action { if ($EventArgs.Data) { $Event.MessageData.Enqueue($EventArgs.Data) } } -MessageData $lines
$p.BeginOutputReadLine()
$err = $p.StandardError.ReadToEndAsync()
$t0 = Get-Date; $answered = $false; $log = @()
while (-not $p.HasExited -and ((Get-Date) - $t0).TotalSeconds -lt 90) {
    $row = Srv access --status pending | Select-Object -First 1
    if ($row -match 'client\s+(\d+).*host\s+(\d+)') { Srv approve $Matches[1] $Matches[2] | Out-Null; "{0,5:N1} s approved" -f ((Get-Date) - $t0).TotalSeconds }
    $l = $null
    while ($lines.TryDequeue([ref]$l)) {
        $log += $l
        if ($l.StartsWith("@status")) { "{0,5:N1} s {1}" -f ((Get-Date) - $t0).TotalSeconds, ($l.Substring(0, [Math]::Min(150, $l.Length))) }
        if (-not $answered -and $l -like '*"event":"monitor_wanted"*') {
            Start-Sleep -Milliseconds 1500  # the user thinks
            $p.StandardInput.WriteLine($Answer); $p.StandardInput.Flush(); $answered = $true
            "{0,5:N1} s answered: {1}" -f ((Get-Date) - $t0).TotalSeconds, $Answer
        }
    }
    Start-Sleep -Milliseconds 200
}
if (-not $p.HasExited) { $p.Kill(); "timeout: killed" }
Start-Sleep -Milliseconds 300
$l = $null; while ($lines.TryDequeue([ref]$l)) { $log += $l }
$log | Set-Content "$d\$Name.log"
$err.Result | Set-Content "$d\$Name.err"
Unregister-Event -SourceIdentifier $job.Name
"exit $($p.ExitCode) after $([int]((Get-Date) - $t0).TotalSeconds) s"
$log | Select-String "host has no monitor|monitors of the host|frames:" | Select-Object -First 5

