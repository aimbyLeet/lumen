# Builds the lumen-server Docker image (with the installer of this version,
# which devices update to: /usr/share/lumen/update) and packs the deployment archive
# <Dist>\lumen-server-<version>.tar.gz: one folder lumen-server-<version>\ with
#   install.sh                 the menu: a clean VPS, a VPS with nginx, a local
#                              network, update, status, removal
#   lumen-server-image.tar     the image
#   compose\public.yml, lan.yml  docker-compose.yml of each mode (install.sh picks)
#   site\                      the landing page and the installer
# How to install it: the user manual, manual\manual_EN.html and manual_RU.html
# ("Quick start: server installation").
# The installer of this version first: deploy\build-installer.ps1.
#
#   deploy\build-server.ps1                                     # dist\, the workspace's version
#   deploy\build-server.ps1 -Version 0.6.8-test -Dist bench-out\srv   # a test build, the release untouched
# Native tools write progress to stderr: exit codes are checked instead.
param(
    [string]$Version = "",
    [string]$Dist = ""
)
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$cargoVersion = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
if (-not $Version) { $Version = $cargoVersion }
if (-not $Dist) { $Dist = "dist" }
# Relative to the workspace (as in the examples above); an absolute path as it is.
if (-not [IO.Path]::IsPathRooted($Dist)) { $Dist = Join-Path $root $Dist }
$Dist = [IO.Path]::GetFullPath($Dist)
New-Item -ItemType Directory -Force $Dist | Out-Null
$name = "lumen-server-$Version"
$stage = "$Dist\$name"
# The installer is the workspace's (a test build reuses the release's).
$setup = "$root\dist\lumen-setup-$cargoVersion.exe"
if (-not (Test-Path $setup)) { throw "no $setup (the landing page offers it): deploy\build-installer.ps1 first" }

# The update door's installer: a folder of its own, the image's build context `update`.
$update = "$Dist\$name-update"
Remove-Item -Recurse -Force $update -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $update | Out-Null
Copy-Item $setup $update
docker build -f "$root\deploy\server\Dockerfile" --build-context "update=$update" -t "lumen-server:$Version" $root
$built = $LASTEXITCODE
Remove-Item -Recurse -Force $update
if ($built) { throw 'docker build failed' }

Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage\compose", "$stage\site\fonts" | Out-Null
docker save -o "$stage\lumen-server-image.tar" "lumen-server:$Version"
if ($LASTEXITCODE) { throw 'docker save failed' }

# Text files: the version put in, LF line ends (they run on Linux).
function Put([string]$from, [string]$to, [hashtable]$more = @{}) {
    $t = (Get-Content -Raw -Encoding utf8 $from).Replace("`r`n", "`n").Replace('@VERSION@', $Version)
    foreach ($k in $more.Keys) { $t = $t.Replace($k, $more[$k]) }
    [IO.File]::WriteAllText($to, $t, (New-Object Text.UTF8Encoding $false))
}
Put "$root\deploy\server\install.sh" "$stage\install.sh"
Put "$root\deploy\server\compose\public.yml" "$stage\compose\public.yml"
Put "$root\deploy\server\compose\lan.yml" "$stage\compose\lan.yml"

Copy-Item $setup "$stage\site\"
Copy-Item "$root\web\admin\public\favicon.png" "$stage\site\icon.png"
# The apps' fonts, with their licences.
Copy-Item "$root\web\client\src\assets\fonts\*" "$stage\site\fonts\"
$size = ([math]::Round((Get-Item $setup).Length / 1MB, 1)).ToString([Globalization.CultureInfo]::InvariantCulture)
$sha = (Get-FileHash -Algorithm SHA256 $setup).Hash.ToLower()
# The landing page names the installer of the workspace's version.
$page = (Get-Content -Raw -Encoding utf8 "$root\deploy\server\site\index.html").
    Replace('@VERSION@', $cargoVersion).Replace('@SIZE@', $size).Replace('@SHA256@', $sha)
[IO.File]::WriteAllText("$stage\site\index.html", $page, (New-Object Text.UTF8Encoding $false))

$archive = "$Dist\$name.tar.gz"
# Packed on Linux: Windows tar would store every file as rw-rw-rw-.
docker run --rm --entrypoint sh -v "${Dist}:/d" "lumen-server:$Version" -c `
    "find /d/$name -type d -exec chmod 755 {} + && find /d/$name -type f -exec chmod 644 {} + && chmod 755 /d/$name/install.sh && tar -czf /d/$name.tar.gz --owner=0 --group=0 -C /d $name"
if ($LASTEXITCODE) { throw 'tar failed' }
Remove-Item -Recurse -Force $stage
Write-Host "$archive ($([math]::Round((Get-Item $archive).Length / 1MB)) MB)"
