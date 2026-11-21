# Pre-commit checks with a short report: one line per step, and on failure only
# the lines that matter (errors, failed tests), capped.
#
#   tools\check.ps1            # fmt, clippy, tests (nextest); web/* only if changed
#   tools\check.ps1 -Web       # also tsc in every web package and npm test in web\client
#   tools\check.ps1 -NoWeb     # Rust only
#   tools\check.ps1 -Full      # tests with `cargo test --workspace` (the reference
#                              # runner) instead of nextest
#   tools\check.ps1 -NoMouse   # without the test that moves the real mouse
#                              # (input's inject_cursor_and_key): while someone
#                              # works at this screen
#   tools\check.ps1 -Lines 200 # more output lines per failed step (default 60)
#
# `cargo fmt --all` rewrites files: the report lists them. Exit code 1 if any
# step failed. Needs cargo-nextest unless -Full (`cargo install cargo-nextest`).
param(
    [switch]$Web,
    [switch]$NoWeb,
    [switch]$Full,
    [switch]$NoMouse,
    [int]$Lines = 60
)
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
$failed = 0

# Runs a command line through cmd (stderr merged as plain text, no PS 5.1
# NativeCommandError records); returns @{ Code; Out; Sec }.
function Invoke-Step([string]$dir, [string]$cmdline) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Push-Location $dir
    $out = @(& cmd /c "$cmdline 2>&1" | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    Pop-Location
    @{ Code = $code; Out = $out; Sec = [int]$sw.Elapsed.TotalSeconds }
}

function Report([string]$name, $r, [string]$summary, [string[]]$detail) {
    if ($r.Code -eq 0) {
        Write-Output ("OK    {0,-22} {1,4} s  {2}" -f $name, $r.Sec, $summary)
    } else {
        $script:failed++
        Write-Output ("FAIL  {0,-22} {1,4} s  exit {2}  {3}" -f $name, $r.Sec, $r.Code, $summary)
        if (-not $detail -or $detail.Count -eq 0) { $detail = $r.Out | Select-Object -Last $Lines }
        $detail | Select-Object -First $Lines | ForEach-Object { "      $_" }
        if ($detail.Count -gt $Lines) { "      ... ($($detail.Count - $Lines) more lines)" }
    }
}

# --- Rust ---------------------------------------------------------------
$r = Invoke-Step $root "cargo fmt --all -- -l"
$files = @($r.Out | Where-Object { $_ -match '\.rs$' })
$s = if ($files.Count) { "reformatted: " + (($files | ForEach-Object { $_.Replace("$root\", "") }) -join ", ") } else { "no changes" }
Report "cargo fmt" $r $s

$r = Invoke-Step $root "cargo clippy --workspace --all-targets --message-format=short -- -D warnings"
$diag = @($r.Out | Where-Object { $_ -match '(^|: )(error|warning)(\[|:)' -and $_ -notmatch '^warning: build failed' })
Report "cargo clippy" $r $(if ($r.Code -eq 0) { "clean" } else { "$($diag.Count) diagnostics" }) $diag

if ($Full) {
    $skip = if ($NoMouse) { " -- --skip inject_cursor_and_key" } else { "" }
    $r = Invoke-Step $root "cargo test --workspace$skip"
    $p = 0; $f = 0; $i = 0
    foreach ($l in $r.Out) {
        if ($l -match 'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored') {
            $p += [int]$matches[1]; $f += [int]$matches[2]; $i += [int]$matches[3]
        }
    }
    $bad = @($r.Out | Where-Object { $_ -match '^test .* FAILED$|panicked at|^error(\[|:)|^\s+-->|^failures:' })
    Report "cargo test" $r "$p passed, $f failed, $i ignored" $bad
} else {
    # No spaces or quotes in the filter: it goes through cmd.
    $skip = if ($NoMouse) { " -E !test(inject_cursor_and_key)" } else { "" }
    $r = Invoke-Step $root "cargo nextest run --workspace --no-fail-fast --cargo-quiet --status-level fail --final-status-level fail$skip"
    $sum = ($r.Out | Where-Object { $_ -match '^\s*Summary' } | Select-Object -Last 1)
    $sum = if ($sum) { ($sum -replace '^\s*Summary \[\s*[\d.]+s\]\s*', '') } else { "no summary" }
    # Compiler warnings before the run are clippy's business: keep from the run
    # header on (all of the output if the run never started, e.g. a build error).
    $start = 0
    for ($k = 0; $k -lt $r.Out.Count; $k++) { if ($r.Out[$k] -match 'Nextest run ID') { $start = $k + 1; break } }
    $bad = @($r.Out[$start..($r.Out.Count - 1)] | Where-Object { $_ -notmatch '^\s*(Starting|Summary)\b' -and $_.Trim() -ne '' })
    Report "cargo nextest" $r $sum $bad
}

# --- Web ----------------------------------------------------------------
if (-not $NoWeb) {
    $pkgs = @("admin", "client", "host")
    if (-not $Web) {
        $changed = @(git -C $root status --porcelain -- web 2>$null | ForEach-Object { "$_" })
        $pkgs = @($pkgs | Where-Object { $p = $_; $changed | Where-Object { $_ -match "web/$p/" } })
    }
    if ($pkgs.Count -eq 0) {
        Write-Output "SKIP  web                           no changes under web\ (-Web to force)"
    }
    foreach ($p in $pkgs) {
        $r = Invoke-Step "$root\web\$p" "npx tsc --noEmit -p ."
        $err = @($r.Out | Where-Object { $_ -match 'error TS\d+' })
        Report "tsc web\$p" $r $(if ($r.Code -eq 0) { "clean" } else { "$($err.Count) errors" }) $err
    }
    if ($pkgs -contains "client") {
        $r = Invoke-Step "$root\web\client" "npm test --silent"
        # node --test, spec reporter: "<info sign> pass 30", "<cross> name" for a failure.
        $pass = ($r.Out | Where-Object { $_ -match '^\S*\s*pass \d+\s*$' } | Select-Object -Last 1) -replace '^\S*\s*', ''
        $fail = ($r.Out | Where-Object { $_ -match '^\S*\s*fail \d+\s*$' } | Select-Object -Last 1) -replace '^\S*\s*', ''
        $cross = [string][char]0x2716
        $bad = @($r.Out | Where-Object { $_.TrimStart().StartsWith($cross) -or $_ -match '^\s*\S*\s*(not ok|failing tests)|^\s+(error|expected|actual|message|code):|AssertionError|Error:' })
        Report "npm test web\client" $r "$pass, $fail" $bad
    }
}

if ($failed) { Write-Output "== $failed step(s) failed"; exit 1 }
Write-Output "== all checks passed"
