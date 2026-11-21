# A session of this machine's host and client through the lumen-server image in Docker
# (dev certificate in the /etc/letsencrypt/live layout, as on the VPS; the request
# is approved by the CLI in the container). Logs and data: bench-out\docker\.
# The image first: deploy\build-server.ps1.
#   tools\docker-session.ps1 -Version 0.5.1
param([string]$Version = "0.5.1", [string]$ClientArgs = "--borderless --frames 600")
$ErrorActionPreference = "Continue"
$root = "C:\dev\lumen"
$rel = "$root\target\release"
$d = "$root\bench-out\docker"
Get-Process lumen-client, lumen-host, lumen-server -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$rel\*" } | Stop-Process -Force -ErrorAction SilentlyContinue
docker rm -f lumen-check 2>$null | Out-Null
Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
New-Item -ItemType Directory "$d\host", "$d\client", "$d\data", "$d\le\live\localhost" | Out-Null

& "$rel\lumen-server.exe" --db "$d\tmp.db" dev-cert --out $d | Out-Null
Copy-Item "$d\cert.pem" "$d\le\live\localhost\fullchain.pem"
Copy-Item "$d\key.pem" "$d\le\live\localhost\privkey.pem"

docker run -d --name lumen-check -p 7443:7443 `
    -e LUMEN_DOMAIN=localhost -e LUMEN_ADMIN_USER=admin -e LUMEN_ADMIN_PASSWORD=checkcheck `
    -v "${d}\data:/data" -v "${d}\le:/etc/letsencrypt:ro" "lumen-server:$Version" | Out-Null
Start-Sleep -Seconds 2
$server = "--server 127.0.0.1:7443 --server-ca `"$d\ca.pem`""

$env:LUMEN_DATA = "$d\host"
$token = (& "$rel\lumen-host.exe" token).Trim()
$hostp = Start-Process "$rel\lumen-host.exe" -WindowStyle Hidden -PassThru `
    -ArgumentList "$server --input dry-run --source synthetic" `
    -RedirectStandardOutput "$d\host.log" -RedirectStandardError "$d\host.err"
$env:LUMEN_DATA = "$d\client"
$client = Start-Process "$rel\lumen-client.exe" -PassThru `
    -ArgumentList "$server --token $token $ClientArgs" `
    -RedirectStandardOutput "$d\client.log" -RedirectStandardError "$d\client.err"
Remove-Item Env:LUMEN_DATA

$approved = $false
for ($i = 0; $i -lt 200 -and -not $approved; $i++) {
    Start-Sleep -Milliseconds 100
    $row = docker exec lumen-check lumen-server access --status pending 2>$null | Select-Object -First 1
    if ($row -match 'client\s+(\d+).*host\s+(\d+)') {
        docker exec lumen-check lumen-server approve $Matches[1] $Matches[2]
        $approved = $true
    }
}
if (-not $approved) { Write-Warning "no access request from the client" }

if (-not $client.WaitForExit(300000)) { Stop-Process $client -Force }
Start-Sleep -Milliseconds 500
Get-Process lumen-host -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$rel\*" } | Stop-Process -Force -ErrorAction SilentlyContinue
"--- server (docker logs)"
docker logs lumen-check 2>&1 | Select-Object -Last 8
docker rm -f lumen-check | Out-Null
"--- client"
Get-Content "$d\client.log", "$d\client.err" -ErrorAction SilentlyContinue | Select-Object -Last 14
