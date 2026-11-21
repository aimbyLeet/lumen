# Installer checks inside Windows Sandbox; lumen.wsb starts it at logon.
#  A. The client, silent, by a standard user (no admin rights): first version, then the second over it
#     while the client runs (/RELAUNCH=1: it must run again, newer).
#  B. The host, silent, as SYSTEM in session 0 (a scheduled task: what the service will do): first
#     version, then the second over it while the service and the tray run.
# The report: C:\LumenOut\results\report.txt (bench-out\sandbox\results on the host machine), with the setup logs.
$ErrorActionPreference = 'Continue'
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $elevated) {
    Start-Process powershell.exe "-NoExit -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs
    exit
}
$setups = Get-ChildItem C:\LumenOut\setup\lumen-setup-*.exe | Sort-Object { [version]($_.BaseName -replace 'lumen-setup-', '') }
if ($setups.Count -lt 2) { throw 'two test installers needed in bench-out\sandbox\setup (tools\installer-sandbox\build.ps1)' }
$v1 = $setups[0].BaseName -replace 'lumen-setup-', ''
$v2 = $setups[1].BaseName -replace 'lumen-setup-', ''
$results = 'C:\LumenOut\results'
Remove-Item "$results\*" -Recurse -Force -ErrorAction SilentlyContinue
# A place every account can read (the mapped folder may not be).
$work = 'C:\Users\Public\lumen'
New-Item -ItemType Directory -Force "$work\logs" | Out-Null
Copy-Item $setups[0].FullName, $setups[1].FullName $work
icacls "$work\logs" /grant 'Everyone:(OI)(CI)F' | Out-Null
$report = "$results\report.txt"
$failed = 0
function Say([string]$text) { $text; Add-Content $report $text }
function Check([string]$what, [bool]$ok, [string]$detail = '') {
    if (-not $ok) { $script:failed++ }
    Say ("[{0}] {1}{2}" -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $what, $(if ($detail) { " ($detail)" } else { '' }))
}
function Uninstall-Entry([Microsoft.Win32.RegistryKey]$hive) {
    $k = $hive.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{22294F4B-89FB-465B-8DD4-DDEE28DFF9CD}_is1')
    if ($k) { $k.GetValue('DisplayVersion') }
}

Say "Lumen installer checks: $v1 -> $v2, $(Get-Date -Format s)"
Say (& "$PSScriptRoot\prep-sandbox.ps1")
$lua = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System').EnableLUA
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Say "Sandbox user: $env:USERNAME, elevated: $admin, UAC (EnableLUA): $lua"

# ---- A. The client by a standard user ----
Say ''
Say '== A. Client: silent, standard user, no UAC =='
$user = 'lumtest'
$password = 'Lumen-test-1!'
net user $user $password /add | Out-Null
$cred = New-Object Management.Automation.PSCredential ($user, (ConvertTo-SecureString $password -AsPlainText -Force))
$isAdminUser = (net localgroup Administrators) -contains $user
Check "test user '$user' is not an administrator" (-not $isAdminUser)
function As-User([string]$exe, [string]$arguments) {
    # Not -Wait: in PowerShell 5.1 it waits for the children too, and /RELAUNCH=1 leaves the client running.
    $p = Start-Process $exe $arguments -Credential $cred -LoadUserProfile -WorkingDirectory $work -PassThru
    $null = $p.Handle  # keeps the handle open: else ExitCode comes back empty
    $p.WaitForExit()
    $p.ExitCode
}
$code = As-User "$work\lumen-setup-$v1.exe" "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client /LOG=$work\logs\client-$v1.log"
Check "client $v1 installed by $user" ($code -eq 0) "exit code $code"
$clientDir = "C:\Users\$user\AppData\Local\Programs\Lumen"
Check 'client in the user''s Programs' (Test-Path "$clientDir\lumen-client-app.exe") $clientDir
Check 'nothing in Program Files' (-not (Test-Path "$env:ProgramFiles\Lumen"))
Check 'no machine-wide uninstall entry' (-not (Uninstall-Entry ([Microsoft.Win32.Registry]::LocalMachine)))
# The runtime on disk, not only its registry key (2026-11-09: the sandbox had the key without the runtime,
# and the apps, running, only showed "WebView2 Runtime not found").
# The version the registry names is the one the apps look for (the sandbox had 157 on disk, 156 registered).
$pv = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue).pv
$wv = @(Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application\$pv\msedgewebview2.exe",
    "C:\Users\$user\AppData\Local\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue)
Check "WebView2 runtime of the registered version ($pv) on disk after the client" ($wv.Count -gt 0) $(if ($wv) { $wv[0].FullName })
Say "  setup log, WebView2: $((Select-String -Path "$work\logs\client-$v1.log" -Pattern 'WebView2' | ForEach-Object { $_.Line.Substring(24) }) -join ' | ')"

# The client runs while its update comes (it would close itself first; force-closing is the worse case).
Start-Process "$clientDir\lumen-client-app.exe" -Credential $cred -WorkingDirectory $clientDir
Start-Sleep 8
$before = Get-Process lumen-client-app -IncludeUserName -ErrorAction SilentlyContinue | Select-Object -First 1
Check 'client running before the update' ($null -ne $before) $(if ($before) { "pid $($before.Id), $($before.UserName)" })
$t0 = Get-Date
$code = As-User "$work\lumen-setup-$v2.exe" "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client /RELAUNCH=1 /LOG=$work\logs\client-$v2.log"
Check "client $v2 installed over $v1 by $user" ($code -eq 0) "exit code $code"
Start-Sleep 8
$after = Get-Process lumen-client-app -IncludeUserName -ErrorAction SilentlyContinue | Where-Object { $_.StartTime -gt $t0 } | Select-Object -First 1
Check 'client running again after the update (/RELAUNCH=1)' ($null -ne $after) $(if ($after) { "pid $($after.Id), $($after.UserName)" })
$old = Get-Process lumen-client-app -ErrorAction SilentlyContinue | Where-Object { $_.StartTime -le $t0 }
Check 'the old client process is gone' ($null -eq $old)
Get-Process lumen-client-app -ErrorAction SilentlyContinue | Stop-Process -Force

# ---- B. The host as SYSTEM ----
Say ''
Say '== B. Host: silent, as SYSTEM in session 0 (as the service will run it) =='
function As-System([string]$exe, [string]$arguments, [int]$timeout = 300) {
    $action = New-ScheduledTaskAction -Execute $exe -Argument $arguments
    Register-ScheduledTask -TaskName LumenSetup -Action $action -User 'NT AUTHORITY\SYSTEM' -RunLevel Highest -Force | Out-Null
    Start-ScheduledTask -TaskName LumenSetup
    $deadline = (Get-Date).AddSeconds($timeout)
    Start-Sleep 2
    while ((Get-ScheduledTask -TaskName LumenSetup).State -eq 'Running' -and (Get-Date) -lt $deadline) { Start-Sleep 1 }
    $r = (Get-ScheduledTaskInfo -TaskName LumenSetup).LastTaskResult
    Unregister-ScheduledTask -TaskName LumenSetup -Confirm:$false
    $r
}
$hostArgs = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS /TYPE=host'
$code = As-System "$work\lumen-setup-$v1.exe" "$hostArgs /LOG=$work\logs\host-$v1.log"
Check "host $v1 installed as SYSTEM" ($code -eq 0) "exit code $code"
$hostDir = "$env:ProgramFiles\Lumen"
Check 'host in Program Files' (Test-Path "$hostDir\lumen-host.exe")
Check "machine-wide uninstall entry $v1" ((Uninstall-Entry ([Microsoft.Win32.Registry]::LocalMachine)) -eq $v1)
Start-Sleep 3
$svc = Get-Service LumenHost -ErrorAction SilentlyContinue
Check 'service LumenHost running' ($svc.Status -eq 'Running') "$($svc.Status)"

# The tray in this session, as the HKLM Run entry starts it at sign-in.
Start-Process "$hostDir\lumen-host-app.exe" '--hidden'
Start-Sleep 8
$tray = Get-Process lumen-host-app -ErrorAction SilentlyContinue
Check 'tray running before the update' ($null -ne $tray)
$svcPid = (Get-CimInstance Win32_Service -Filter "Name='LumenHost'").ProcessId
$code = As-System "$work\lumen-setup-$v2.exe" "$hostArgs /LOG=$work\logs\host-$v2.log"
Check "host $v2 installed over $v1 as SYSTEM" ($code -eq 0) "exit code $code"
Check "machine-wide uninstall entry $v2" ((Uninstall-Entry ([Microsoft.Win32.Registry]::LocalMachine)) -eq $v2)
Start-Sleep 3
$svc = Get-Service LumenHost -ErrorAction SilentlyContinue
$newPid = (Get-CimInstance Win32_Service -Filter "Name='LumenHost'").ProcessId
Check 'service running again, a new process' ($svc.Status -eq 'Running' -and $newPid -ne $svcPid) "$($svc.Status), pid $svcPid -> $newPid"
Check 'tray closed by the setup (the service will start it: step 4)' ($null -eq (Get-Process lumen-host-app -ErrorAction SilentlyContinue))
$start = (Get-CimInstance Win32_Service -Filter "Name='LumenHost'").StartMode
Say "service start type: $start"

# ---- Done ----
Copy-Item "$work\logs\*" $results
Copy-Item 'C:\ProgramData\Lumen\host\*.log' $results -ErrorAction SilentlyContinue
Say ''
Say $(if ($failed) { "FAILED: $failed check(s). Logs: bench-out\sandbox\results" } else { 'ALL CHECKS PASSED' })
Say ''
Say 'By hand (the wizard): run C:\Users\Public\lumen\lumen-setup-*.exe from Explorer,'
Say '  1) choose Client: no UAC, it goes to %LOCALAPPDATA%\Programs\Lumen;'
Say '  2) run again, choose Host: UAC (or none if elevated already), the wizard goes on without the type page.'
& "$PSScriptRoot\finished.ps1" 'lumen.wsb' $failed 'bench-out\sandbox\results\report.txt'
