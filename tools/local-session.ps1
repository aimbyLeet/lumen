# A session on this machine through a local lumen-server (development and checks).
#
# Starts the server (development certificate), a host and a client, each with
# its own data folder (LUMEN_DATA), approves the client's request and waits for
# the client to exit. Logs and data: bench-out\local\.
#
#   tools\local-session.ps1 -HostArgs "--source synthetic" -ClientArgs "--borderless --frames 900 --barcode-every 30"
#   tools\local-session.ps1 -HostArgs "--source synthetic --seconds 18 --dump-stream bench-out\host.hevc" `
#       -ClientArgs "--borderless --frames 900 --sim-loss 5 --frame-hashes bench-out\client.hash"
#
# The host runs with --input dry-run unless HostArgs says otherwise (on one
# machine an injected click lands in the client's window).
#
# -SecondClientArgs: 3 s (-SecondClientDelaySec) after the approval, a second
# copy of the same client (same data folder, the same device) starts; its
# output goes to client2.log.
param(
    [string]$HostArgs = "--source synthetic",
    [string]$ClientArgs = "--borderless --frames 600",
    [string]$SecondClientArgs = "",
    [int]$SecondClientDelaySec = 3,
    [int]$TimeoutSec = 300
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$rel = "$root\target\release"
$d = "$root\bench-out\local"
Get-Process lumen-client, lumen-host, lumen-server -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$rel\*" } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 300
Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
New-Item -ItemType Directory "$d\host", "$d\client" | Out-Null

function Srv { & "$rel\lumen-server.exe" --db "$d\lumen.db" @args }
Srv dev-cert --out $d | Out-Null
$srv = Start-Process "$rel\lumen-server.exe" -WindowStyle Hidden -PassThru `
    -ArgumentList "--db `"$d\lumen.db`" serve --listen 127.0.0.1:7443 --cert `"$d\cert.pem`" --key `"$d\key.pem`"" `
    -RedirectStandardOutput "$d\server.log" -RedirectStandardError "$d\server.err"
Start-Sleep -Milliseconds 500
$server = "--server 127.0.0.1:7443 --server-ca `"$d\ca.pem`""

$env:LUMEN_DATA = "$d\host"
$token = (& "$rel\lumen-host.exe" token).Trim()
$hostp = Start-Process "$rel\lumen-host.exe" -WindowStyle Hidden -PassThru `
    -ArgumentList "$server --input dry-run $HostArgs" `
    -RedirectStandardOutput "$d\host.log" -RedirectStandardError "$d\host.err"
$env:LUMEN_DATA = "$d\client"
$client = Start-Process "$rel\lumen-client.exe" -PassThru `
    -ArgumentList "$server --token $token $ClientArgs" `
    -RedirectStandardOutput "$d\client.log" -RedirectStandardError "$d\client.err"
Remove-Item Env:LUMEN_DATA

# Approve the client's request as the admin would.
$approved = $false
for ($i = 0; $i -lt 200 -and -not $approved; $i++) {
    Start-Sleep -Milliseconds 100
    $row = Srv access --status pending | Select-Object -First 1
    if ($row -match 'client\s+(\d+).*host\s+(\d+)') {
        Srv approve $Matches[1] $Matches[2]
        $approved = $true
    }
}
if (-not $approved) { Write-Warning "no access request from the client (see $d\client.log)" }

$second = $null
if ($SecondClientArgs) {
    Start-Sleep -Seconds $SecondClientDelaySec
    $env:LUMEN_DATA = "$d\client"
    $second = Start-Process "$rel\lumen-client.exe" -PassThru `
        -ArgumentList "$server --token $token $SecondClientArgs" `
        -RedirectStandardOutput "$d\client2.log" -RedirectStandardError "$d\client2.err"
    Remove-Item Env:LUMEN_DATA
}

if (-not $client.WaitForExit($TimeoutSec * 1000)) { Stop-Process $client -Force }
if ($second -and -not $second.WaitForExit(30000)) { Stop-Process $second -Force }
Start-Sleep -Milliseconds 500
if (-not $hostp.HasExited) {
    # A host with --seconds exits by itself after its session.
    $hostp.WaitForExit(3000) | Out-Null
}
Get-Process lumen-host, lumen-server -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$rel\*" } | Stop-Process -Force -ErrorAction SilentlyContinue
"--- client ($d\client.log)"
Get-Content "$d\client.log", "$d\client.err" -ErrorAction SilentlyContinue | Select-Object -Last 14
