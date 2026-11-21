# For checking the host's update on a real Windows — Windows Sandbox has no
# console session for the host: Lumen built as three versions (the workspace's version changed for a moment,
# then put back byte for byte), their installers, lumen-client.exe of the middle one (a newer client),
# lumen-server.exe of this code. Out: bench-out\sandbox\host-update\ (setup\, client\, server\).
# The service holds target\release\lumen-host.exe: stop it first. Needs Inno Setup and the VDD package.
param([string[]]$Versions = @('0.6.90', '0.6.91', '0.6.92'), [string]$VddPackage = 'C:\dev\lumen-vdd\build\package')
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot\common.ps1"
$out = "$root\bench-out\sandbox\host-update"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$out\setup", "$out\server", "$out\client" | Out-Null

foreach ($ui in 'host', 'client') {
    Push-Location "$root\web\$ui"
    try { npm run build; if ($LASTEXITCODE) { throw "npm run build ($ui) failed" } } finally { Pop-Location }
}
cargo build --release -p lumen-server --manifest-path "$root\Cargo.toml"
if ($LASTEXITCODE) { throw 'cargo build lumen-server failed' }
Copy-Item "$bin\lumen-server.exe", $crt.FullName "$out\server\"

Invoke-AsVersions $Versions {
    param($v)
    cargo build --release -p lumen-host -p lumen-host-app -p lumen-client -p lumen-client-app --manifest-path "$root\Cargo.toml"
    if ($LASTEXITCODE) { throw "cargo build $v failed" }
    Build-Setup $v "$out\setup" $VddPackage
    if ($v -eq $Versions[1]) { Copy-Item "$bin\lumen-client.exe", $crt.FullName "$out\client\" }
    "built $out\setup\lumen-setup-$v.exe"
}
