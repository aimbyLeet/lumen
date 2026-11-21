# The end of a sandbox check, plain to see: a command window that says FINISHED, the result and the report.
# The run-*.ps1 scripts call it last: & "$PSScriptRoot\finished.ps1" <name> <failed count> <report on the host>.
param([string]$Name, [int]$Failed, [string]$Report)
$result = if ($Failed) { "FAILED: $Failed check(s)" } else { 'ALL OK' }
$color = if ($Failed) { '4F' } else { '2F' }
Start-Process cmd.exe "/k title FINISHED - $Name & color $color & echo. & echo   FINISHED: $Name & echo. & echo   $result & echo   Report: $Report & echo. & echo   The sandbox can be closed now."
