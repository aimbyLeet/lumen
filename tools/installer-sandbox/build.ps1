# Test installers for the sandbox checks: the
# same programs (target\release, built before) under two setup versions, so that
# the second installs over the first as an update would. ISCC only, no cargo.
# Out: bench-out\sandbox\<Out>\lumen-setup-<version>.exe (Out: setup; purge for purge.wsb).
param([string[]]$Versions = @('0.6.90', '0.6.91'), [string]$VddPackage = 'C:\dev\lumen-vdd\build\package', [string]$Out = 'setup')
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot\common.ps1"
$out = "$root\bench-out\sandbox\$Out"
New-Item -ItemType Directory -Force $out, "$root\bench-out\sandbox\results" | Out-Null
foreach ($v in $Versions) {
    Build-Setup $v $out $VddPackage
    "$out\lumen-setup-$v.exe"
}
