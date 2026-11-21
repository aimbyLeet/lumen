# "Delete user data" on uninstall, inside Windows Sandbox (purge.wsb starts it at logon).
# Before: an installer in bench-out\sandbox\purge (tools\installer-sandbox\build.ps1 -Out purge, one version).
#  A. The client by a standard user: uninstall without /PURGE keeps the data; with /PURGE=1 the client's
#     data goes, a host's files in the same folders stay.
#  B. The host as administrator: /PURGE=1 removes %ProgramData%\Lumen and Lumen Host's files in every
#     profile, the client's files stay.
#  C. Left for a look: the client installed again - uninstall it from Settings -> Apps to see the box.
# The report: C:\LumenOut\purge\report.txt (bench-out\sandbox\purge on the host machine).
$ErrorActionPreference = 'Continue'
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $elevated) {
    Start-Process powershell.exe "-NoExit -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs
    exit
}
$setup = Get-ChildItem C:\LumenOut\purge\lumen-setup-*.exe | Select-Object -First 1
if (-not $setup) { throw 'no installer in bench-out\sandbox\purge' }
$report = 'C:\LumenOut\purge\report.txt'
Remove-Item $report -ErrorAction SilentlyContinue
$work = 'C:\Users\Public\lumen'
New-Item -ItemType Directory -Force "$work\logs" | Out-Null
# Its own name: the earlier sandbox runs, which Defender let through, kept it.
$exe = "$work\$($setup.Name)"
Copy-Item $setup.FullName $exe
icacls $work /grant 'Everyone:(OI)(CI)F' | Out-Null
$failed = 0
function Say([string]$text) { $text; Add-Content $report $text }
function Check([string]$what, [bool]$ok) {
    if (-not $ok) { $script:failed++ }
    Say ("[{0}] {1}" -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $what)
}
function Put([string]$file) {
    New-Item -ItemType Directory -Force (Split-Path $file) | Out-Null
    Set-Content $file 'x'
}
# The uninstaller runs a copy of itself from %TEMP% and returns at once: wait until its folder is gone.
function Wait-Gone([string]$dir) {
    for ($i = 0; $i -lt 60 -and (Test-Path "$dir\unins000.exe"); $i++) { Start-Sleep 1 }
    Start-Sleep 2
}

Say "Lumen uninstall purge checks: $($setup.Name), $(Get-Date -Format s)"

Say (& "$PSScriptRoot\prep-sandbox.ps1")
# A block, if any, shows as "start failed: ... Application Control policy" in A and B.

# ---- A. The client ----
Say ''
Say '== A. Client, standard user =='
$user = 'lumtest'
$password = 'Lumen-test-1!'
net user $user $password /add | Out-Null
$cred = New-Object Management.Automation.PSCredential ($user, (ConvertTo-SecureString $password -AsPlainText -Force))
function As-User([string]$exe, [string]$arguments) {
    try {
        $p = Start-Process $exe $arguments -Credential $cred -LoadUserProfile -WorkingDirectory $work -PassThru -ErrorAction Stop
    } catch {
        Say "  start failed: $exe - $($_.Exception.Message)"
        return 'none'
    }
    $null = $p.Handle
    $p.WaitForExit()
    $p.ExitCode
}
$profile = "C:\Users\$user"
$roam = "$profile\AppData\Roaming"
$local = "$profile\AppData\Local"
$app = "$local\Programs\Lumen"
$clientFiles = @("$roam\Lumen\client.key", "$roam\Lumen\client-app.json", "$roam\Lumen\servers\a.creds",
    "$roam\Lumen\views\1.json", "$local\Lumen\logs\client-2026.log", "$local\Lumen\updates\lumen-setup-1.exe",
    "$local\io.github.aimbyleet.lumen.client\EBWebView\f")
$hostFiles = @("$roam\Lumen\host-app.json", "$local\Lumen\logs\host-2026.log", "$local\io.github.aimbyleet.lumen.host\EBWebView\f")

$code = As-User $exe "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client /LOG=$work\logs\client-1.log"
Check "client installed (exit $code)" ($code -eq 0 -and (Test-Path "$app\lumen-client-app.exe"))
$clientFiles + $hostFiles | ForEach-Object { Put $_ }
$code = As-User "$app\unins000.exe" "/VERYSILENT /SUPPRESSMSGBOXES /LOG=$work\logs\client-uninstall-keep.log"
Wait-Gone $app
Check 'client uninstalled' (-not (Test-Path "$app\lumen-client-app.exe"))
Check 'without /PURGE: all the data stays' (-not ($clientFiles + $hostFiles | Where-Object { -not (Test-Path $_) }))

$code = As-User $exe "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client /LOG=$work\logs\client-2.log"
Check "client installed again (exit $code)" ($code -eq 0)
$code = As-User "$app\unins000.exe" "/VERYSILENT /SUPPRESSMSGBOXES /PURGE=1 /LOG=$work\logs\client-uninstall-purge.log"
Wait-Gone $app
$left = $clientFiles | Where-Object { Test-Path $_ }
Check "/PURGE=1: the client's data is gone$(if ($left) { ' - left: ' + ($left -join ', ') })" (-not $left)
Check '/PURGE=1: the client''s folders are gone' (-not (Test-Path "$roam\Lumen\servers") -and -not (Test-Path "$local\Lumen\updates") -and -not (Test-Path "$local\io.github.aimbyleet.lumen.client"))
$lost = $hostFiles | Where-Object { -not (Test-Path $_) }
Check "/PURGE=1: the host's files stay$(if ($lost) { ' - lost: ' + ($lost -join ', ') })" (-not $lost)

# ---- B. The host ----
Say ''
Say '== B. Host, administrator =='
$hostApp = "$env:ProgramFiles\Lumen"
try {
    $code = (Start-Process $exe "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS /TYPE=host /LOG=$work\logs\host-1.log" -PassThru -Wait -ErrorAction Stop).ExitCode
} catch {
    Say "  start failed: $exe - $($_.Exception.Message)"
    $code = 'none'
}
Check "host installed (exit $code)" ($code -eq 0 -and (Test-Path "$hostApp\lumen-host.exe"))
Start-Sleep 5
$machine = @("$env:ProgramData\Lumen\host\fake.key", "$env:ProgramData\Lumen\monitors.json", "$env:ProgramData\Lumen\logs\service-1.log")
$machine | ForEach-Object { Put $_ }
# The client's files (from A, gone) again: they must stay now.
$clientKeep = @("$roam\Lumen\client.key", "$local\Lumen\logs\client-2026.log")
$clientKeep | ForEach-Object { Put $_ }
$me = "$env:USERPROFILE\AppData"
$hostFiles2 = $hostFiles + @("$me\Roaming\Lumen\host-app.json", "$me\Local\Lumen\logs\host-2026.log", "$me\Local\io.github.aimbyleet.lumen.host\EBWebView\f")
$hostFiles2 | ForEach-Object { Put $_ }
Start-Process "$hostApp\unins000.exe" "/VERYSILENT /SUPPRESSMSGBOXES /PURGE=1 /LOG=$work\logs\host-uninstall-purge.log" -Wait
Wait-Gone $hostApp
Check 'host uninstalled' (-not (Test-Path "$hostApp\lumen-host.exe"))
Check 'service removed' (-not (Get-Service LumenHost -ErrorAction SilentlyContinue))
Check '/PURGE=1: %ProgramData%\Lumen is gone' (-not (Test-Path "$env:ProgramData\Lumen"))
$left = $hostFiles2 | Where-Object { Test-Path $_ }
Check "/PURGE=1: Lumen Host's files gone in every profile$(if ($left) { ' - left: ' + ($left -join ', ') })" (-not $left)
$lost = $clientKeep | Where-Object { -not (Test-Path $_) }
Check "/PURGE=1: the client's files stay$(if ($lost) { ' - lost: ' + ($lost -join ', ') })" (-not $lost)

Copy-Item "$work\logs\*" C:\LumenOut\purge\ -ErrorAction SilentlyContinue
# What Defender did here, if anything.
Get-MpThreatDetection -ErrorAction SilentlyContinue | ForEach-Object { Say "  Defender: $($_.ThreatID) $($_.Resources -join ', ')" }
Get-WinEvent -LogName 'Microsoft-Windows-Windows Defender/Operational' -MaxEvents 20 -ErrorAction SilentlyContinue |
    Where-Object { $_.Id -in 1006, 1007, 1008, 1015, 1116, 1117, 1121, 1122 } |
    ForEach-Object { Say "  Defender event $($_.Id): $(($_.Message -split "`n")[0..6] -join ' | ')" }
Say ''
Say $(if ($failed) { "FAILED: $failed" } else { 'ALL OK' })

# ---- C. A look at the page ----
$code = (Start-Process $exe "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client /LANG=ru" -PassThru -Wait).ExitCode
Say ''
Say "For a look (exit $code): uninstall Lumen from Settings -> Apps - the page 'User data' (in Russian), the box off."
& "$PSScriptRoot\finished.ps1" 'purge.wsb' $failed 'bench-out\sandbox\purge\report.txt'
