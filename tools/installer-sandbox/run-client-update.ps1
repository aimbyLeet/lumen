# The client's update in Windows Sandbox; client-update.wsb starts it at logon.
# Before: tools\installer-sandbox\build-client-update.ps1 (bench-out\sandbox\update: two client versions, the server).
#  1. Lumen Client of the old version, installed for this user, its server set; the server offers the new one:
#     the client updates itself at start (download, installer, start again) — the new version runs.
#  2. The server's installer damaged after the server hashed it: the client refuses it and keeps running.
# The report: C:\LumenOut\results\client-update.txt (bench-out\sandbox\results on the host machine).
#
# Two layers (2026-11-09). The sandbox's own account is an administrator with UAC off: WebView2's bootstrapper
# then installs per machine, which fails in the sandbox (0x80040C02), and the client cannot open its window,
# so it never updates. A standard user gets WebView2 per user (it works, lumen.wsb). So this account prepares
# the sandbox, makes the user lumtest and runs the check itself as lumtest (-Inner), in folders it can reach.
param([switch]$Inner)
$ErrorActionPreference = 'Continue'
$shared = 'C:\Users\Public\lumen-up-run'
if (-not $Inner) {
    $out = 'C:\LumenOut\results'
    New-Item -ItemType Directory -Force $out, "$shared\results" | Out-Null
    Remove-Item "$out\client-update.txt", "$shared\results\*" -Force -ErrorAction SilentlyContinue
    $prep = & "$PSScriptRoot\prep-sandbox.ps1"
    Copy-Item 'C:\LumenOut\update' "$shared\src" -Recurse -Force
    Copy-Item $PSCommandPath "$shared\run-client-update.ps1" -Force
    Set-Content "$shared\results\prep.txt" $prep
    icacls $shared /grant 'Everyone:(OI)(CI)F' /T | Out-Null
    $user = 'lumtest'
    $password = 'Lumen-test-1!'
    net user $user $password /add | Out-Null
    $cred = New-Object Management.Automation.PSCredential ($user, (ConvertTo-SecureString $password -AsPlainText -Force))
    "Running the check as $user (its own window)..."
    $p = Start-Process powershell.exe "-ExecutionPolicy Bypass -File `"$shared\run-client-update.ps1`" -Inner" `
        -Credential $cred -LoadUserProfile -WorkingDirectory $shared -PassThru
    $null = $p.Handle; $p.WaitForExit()
    Copy-Item "$shared\results\*" $out -Force -ErrorAction SilentlyContinue
    $text = Get-Content "$out\client-update.txt" -ErrorAction SilentlyContinue
    $text
    $failed = if ($text -match 'ALL CHECKS PASSED') { 0 } else { [Math]::Max(1, @($text | Select-String '^\[FAIL\]').Count) }
    & "$PSScriptRoot\finished.ps1" 'client-update.wsb' $failed 'bench-out\sandbox\results\client-update.txt'
    exit
}
$src = "$shared\src"
$results = "$shared\results"
$report = "$results\client-update.txt"
$failed = 0
function Say([string]$text) { $text; Add-Content $report $text }
function Check([string]$what, [bool]$ok, [string]$detail = '') {
    if (-not $ok) { $script:failed++ }
    Say ("[{0}] {1}{2}" -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $what, $(if ($detail) { " ($detail)" } else { '' }))
}
$setups = Get-ChildItem "$src\setup\lumen-setup-*.exe" | Sort-Object { [version]($_.BaseName -replace 'lumen-setup-', '') }
$old = $setups[0].BaseName -replace 'lumen-setup-', ''
$new = $setups[1].BaseName -replace 'lumen-setup-', ''
Say "Lumen Client update: $old -> $new, $(Get-Date -Format s), as $env:USERNAME"
Say (Get-Content "$results\prep.txt")

$work = 'C:\Users\Public\lumen-up'
New-Item -ItemType Directory -Force "$work\offer", "$work\srv" | Out-Null
Copy-Item "$src\server\*" $work
& "$work\lumen-server.exe" dev-cert --out "$work\srv" | Out-Null
$server = $null
function Start-Server {
    $script:server = Start-Process "$work\lumen-server.exe" "--db `"$work\srv\lumen.db`" serve --listen 127.0.0.1:7443 --cert `"$work\srv\cert.pem`" --key `"$work\srv\key.pem`" --update-dir `"$work\offer`"" `
        -PassThru -WindowStyle Hidden -RedirectStandardOutput "$work\srv\server-$(Get-Random).log" -RedirectStandardError "$work\srv\server-errors-$(Get-Random).log"
    Start-Sleep 2
}
function Installed-Version {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{22294F4B-89FB-465B-8DD4-DDEE28DFF9CD}_is1')
    if ($k) { $k.GetValue('DisplayVersion') }
}
$exe = "$env:LOCALAPPDATA\Programs\Lumen\lumen-client-app.exe"

# ---- 1. The old client updates itself at start.
Copy-Item "$src\setup\lumen-setup-$new.exe" "$work\offer\"
Start-Server
$p = Start-Process "$src\setup\lumen-setup-$old.exe" "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client /LOG=$results\client-$old.log" -PassThru
$null = $p.Handle; $p.WaitForExit()
Check "client $old installed for this user" ((Installed-Version) -eq $old) "exit $($p.ExitCode)"
New-Item -ItemType Directory -Force "$env:APPDATA\Lumen" | Out-Null
$ca = "$work\srv\ca.pem" -replace '\\', '\\'
Set-Content "$env:APPDATA\Lumen\client-app.json" "{`"name`":`"sandbox`",`"server`":`"127.0.0.1:7443`",`"server_ca`":`"$ca`",`"auto_update`":true}" -Encoding ascii
$t0 = Get-Date
Start-Process $exe
$deadline = (Get-Date).AddSeconds(90)
while ((Installed-Version) -ne $new -and (Get-Date) -lt $deadline) { Start-Sleep 1 }
Check "updated to $new by itself" ((Installed-Version) -eq $new) "$([int]((Get-Date) - $t0).TotalSeconds) s"
Start-Sleep 8
$running = Get-Process lumen-client-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }
Check 'the client runs again' ($null -ne $running)
# The version the program was built with: a string inside it.
Check "the program is $new" ([IO.File]::ReadAllText($exe, [Text.Encoding]::ASCII).Contains($new))

# ---- 2. A damaged installer: refused, the client keeps running.
$running | Stop-Process -Force
Stop-Process -Id $server.Id -Force
Remove-Item "$work\offer\*"
$bad = "$work\offer\lumen-setup-0.6.99.exe"
Copy-Item "$src\setup\lumen-setup-$new.exe" $bad
Start-Server
# After the server took its SHA-256: one byte changed, the size the same.
$bytes = [IO.File]::ReadAllBytes($bad); $bytes[[int]($bytes.Length / 2)] = $bytes[[int]($bytes.Length / 2)] -bxor 0xFF; [IO.File]::WriteAllBytes($bad, $bytes)
Start-Process $exe
Start-Sleep 25
$still = Get-Process lumen-client-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }
Check 'damaged installer refused: still the same version' ((Installed-Version) -eq $new)
Check 'the client keeps running' ($null -ne $still)
$log = Get-ChildItem "$env:LOCALAPPDATA\Lumen\logs\client*.log" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
$sha = $log -and (Select-String -Path $log.FullName -Pattern 'SHA-256' -Quiet)
Check 'the log says why (SHA-256)' ([bool]$sha)

Copy-Item "$env:LOCALAPPDATA\Lumen\logs\*" $results -ErrorAction SilentlyContinue
Copy-Item "$work\srv\server-*.log" $results -ErrorAction SilentlyContinue
Copy-Item "$env:LOCALAPPDATA\Lumen\updates\install.log" $results -ErrorAction SilentlyContinue
Say ''
Say $(if ($failed) { "FAILED: $failed check(s)" } else { 'ALL CHECKS PASSED' })
Get-Process lumen-client-app -ErrorAction SilentlyContinue | Stop-Process -Force
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
