# What the build scripts here share (dot-sourced: . "$PSScriptRoot\common.ps1"): the workspace, Inno Setup's
# compiler, the C runtime of Visual Studio, an installer built, and Lumen built as another version for a moment.
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$bin = "$root\target\release"
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
# Windows Sandbox has no C runtime: the installer brings it for the apps, lumen-server.exe takes it along.
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT\vcruntime140.dll" | Sort-Object FullName | Select-Object -Last 1

# lumen-setup-<Version>.exe into $OutDir, of the programs in target\release as they are.
function Build-Setup([string]$Version, [string]$OutDir, [string]$VddPackage) {
    & $iscc /Q "/DAppVersion=$Version" "/DBinDir=$bin" "/DOutDir=$OutDir" "/DCrtDir=$($crt.DirectoryName)" "/DVddDir=$VddPackage" "$root\deploy\installer\lumen.iss"
    if ($LASTEXITCODE) { throw "ISCC $Version failed" }
}

# Runs $Each with each version as the workspace's (Cargo.toml changed for the while); after, Cargo.toml and
# Cargo.lock are back as they were, byte for byte, whatever happened.
function Invoke-AsVersions([string[]]$Versions, [scriptblock]$Each) {
    $manifest = Get-Content -Raw "$root\Cargo.toml"
    $lock = Get-Content -Raw "$root\Cargo.lock"
    $current = ([regex]'(?m)^version = "(.+)"').Match($manifest).Groups[1].Value
    try {
        foreach ($v in $Versions) {
            [IO.File]::WriteAllText("$root\Cargo.toml", ([regex]'(?m)^version = ".+"').Replace($manifest, "version = `"$v`"", 1))
            # The apps' build scripts run again for the new version.
            (Get-Item "$root\apps\client-app\build.rs").LastWriteTime = Get-Date
            (Get-Item "$root\apps\host-app\build.rs").LastWriteTime = Get-Date
            & $Each $v
        }
    } finally {
        [IO.File]::WriteAllText("$root\Cargo.toml", $manifest)
        [IO.File]::WriteAllText("$root\Cargo.lock", $lock)
        "the workspace's version is $current again"
    }
}
