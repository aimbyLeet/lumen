# Windows Sandbox made fit for the checks; the run-*.ps1 scripts call it first. Not elevated: runs itself
# elevated and waits. Prints what it found and did. The sandbox is thrown away, so both changes are harmless.
#  1. Smart App Control off: since 2026-11-08 the sandbox starts with it on, and it blocks every unsigned
#     installer ("An Application Control policy has blocked this file"). No restart: CiTool -r.
#  2. WebView2's registration removed. The sandbox shows the host's Program Files, WebView2 included, with a
#     registry of its own, older image (2026-11-09: 156.0.4314.50 registered, 157.0.4341.42 on disk), and the
#     apps cannot use that runtime ("Could not find the WebView2 Runtime", also with the registry set to the
#     version on disk). Without the registration Lumen's setup installs WebView2 itself — which checks that path:
#     per user (the client) it works; per machine (the host, as SYSTEM) Microsoft's bootstrapper fails in the
#     sandbox (0x80040C02, also when run here as administrator), so the host's tray there shows that error.
$ci = 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy'
$wv = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$sacOn = (Get-ItemProperty $ci -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState -ne 0
$registered = (Get-ItemProperty $wv -ErrorAction SilentlyContinue).pv

if ($sacOn -or $registered) {
    if (-not $elevated) {
        Start-Process powershell.exe "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs -Wait
    } else {
        if ($sacOn) {
            Set-ItemProperty $ci VerifiedAndReputablePolicyState 0 -Type DWord
            CiTool.exe -r -json | Out-Null
        }
        Remove-Item $wv -Recurse -Force -ErrorAction SilentlyContinue
    }
}
"Smart App Control (0 off, 1 on, 2 evaluation): $((Get-ItemProperty $ci -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState); " +
"WebView2 registration: $(if (Test-Path $wv) { 'still there' } else { 'removed, the setup installs it' }) (was $registered)"
