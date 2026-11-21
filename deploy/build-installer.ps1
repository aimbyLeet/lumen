# Builds the apps (release) and the one installer: dist\lumen-setup-<version>.exe
# (host or client, chosen while installing). The user manual is manual\manual_EN.html
# and manual_RU.html in the repository (not copied to dist).
# Needs Node.js (the apps' UIs), Inno Setup 6 (winget install JRSoftware.InnoSetup)
# and Visual Studio's C++ tools (vcruntime140.dll goes next to the programs).
# The Lumen VDD driver package (virtual monitors, the host's option) is the signed
# release package in third_party\lumen-vdd (checked against its SHA256SUMS);
# -VddPackage <lumen-vdd>\build\package takes one built from its own repository.
# A running service holds target\release\lumen-host.exe: stop it first (tools\dev-service.ps1).
# Native tools write progress to stderr: exit codes are checked instead.
param([string]$VddPackage = '')
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
if (-not $VddPackage) {
    $VddPackage = "$root\third_party\lumen-vdd"
    foreach ($line in Get-Content "$VddPackage\SHA256SUMS") {
        $hash, $name = $line -split '\s+\*?', 2
        if ((Get-FileHash "$VddPackage\$name" -Algorithm SHA256).Hash -ne $hash) {
            throw "Lumen VDD package: $name does not match SHA256SUMS"
        }
    }
}
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
$bin = "$root\target\release"
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT\vcruntime140.dll" | Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw 'vcruntime140.dll not found in the Visual Studio redist folder' }
# lumen-vdd-ctl.exe stays out: a tool for checks by hand, not for users.
foreach ($f in 'lumen_vdd.inf', 'lumen_vdd.cat', 'lumen_vdd.dll', 'lumen-vdd.cer', 'lumen-vdd-setup.exe') {
    if (-not (Test-Path "$VddPackage\$f")) { throw "Lumen VDD package: $VddPackage\$f not found (build it in the lumen-vdd repository)" }
}
$vddVersion = (Select-String -Path "$VddPackage\lumen_vdd.inf" -Pattern '^DriverVer\s*=\s*[^,]+,\s*(\S+)').Matches[0].Groups[1].Value
"Lumen VDD package: $VddPackage (driver $vddVersion)"

# The UIs first: Tauri builds them into the apps.
foreach ($ui in 'host', 'client') {
    Push-Location "$root\web\$ui"
    try {
        npm ci --no-fund --no-audit
        if ($LASTEXITCODE) { throw "npm ci ($ui) failed" }
        npm run build
        if ($LASTEXITCODE) { throw "npm run build ($ui) failed" }
    } finally { Pop-Location }
}
# Tauri reads the built UI at compile time; make sure the apps see a new one.
(Get-Item "$root\apps\host-app\build.rs").LastWriteTime = Get-Date
(Get-Item "$root\apps\client-app\build.rs").LastWriteTime = Get-Date
# Source paths end up in the binaries (panic locations): no user profile or checkout path in them.
# The encoded form keeps paths with spaces whole (arguments are separated by 0x1F).
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { "$env:USERPROFILE\.cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = (
    "--remap-path-prefix=$cargoHome=cargo", "--remap-path-prefix=$env:USERPROFILE=user", "--remap-path-prefix=$root=lumen"
) -join [char]0x1F
cargo build --release --locked -p lumen-host -p lumen-client -p lumen-host-app -p lumen-client-app --manifest-path "$root\Cargo.toml"
if ($LASTEXITCODE) { throw 'cargo build failed' }

# WebView2 bootstrapper (installs the runtime where Windows lacks it; needs the internet).
$wv2 = "$bin\MicrosoftEdgeWebview2Setup.exe"
if (-not (Test-Path $wv2)) {
    Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $wv2
}

& $iscc /Q "/DAppVersion=$version" "/DBinDir=$bin" "/DOutDir=$root\dist" "/DCrtDir=$($crt.DirectoryName)" "/DVddDir=$VddPackage" "$root\deploy\installer\lumen.iss"
if ($LASTEXITCODE) { throw 'ISCC failed' }
Get-ChildItem "$root\dist\lumen-setup-$version.exe" | ForEach-Object { "$($_.FullName) ($([math]::Round($_.Length / 1MB, 1)) MB)" }
