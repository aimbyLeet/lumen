# The client's update end to end, in Windows Sandbox: Lumen Client built
# as two versions (the workspace's version changed for a moment, then put back), their installers, and
# lumen-server.exe of this code. Out: bench-out\sandbox\update\ (setup\, server\). Then client-update.wsb.
# Needs: the UIs built (npm run build in web\client and web\host), Inno Setup, the VDD package (see build.ps1).
param([string]$Old = '0.6.90', [string]$New = '0.6.91', [string]$VddPackage = 'C:\dev\lumen-vdd\build\package')
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot\common.ps1"
$out = "$root\bench-out\sandbox\update"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$out\setup", "$out\server" | Out-Null

cargo build --release -p lumen-server --manifest-path "$root\Cargo.toml"
if ($LASTEXITCODE) { throw 'cargo build lumen-server failed' }
Copy-Item "$bin\lumen-server.exe", $crt.FullName "$out\server\"

Invoke-AsVersions $Old, $New {
    param($v)
    cargo build --release -p lumen-client -p lumen-client-app --manifest-path "$root\Cargo.toml"
    if ($LASTEXITCODE) { throw "cargo build $v failed" }
    # The installer takes the host's programs too: those of the workspace, as they are.
    Build-Setup $v "$out\setup" $VddPackage
    "built $out\setup\lumen-setup-$v.exe"
}
