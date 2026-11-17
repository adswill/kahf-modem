# KAHF build check for Windows (x64 and ARM64).
# Runs: cargo check --workspace, then cargo build -p kahf-cli.
# Usage (from PowerShell, in the repo folder):  .\scripts\build-check.ps1
# Logs go to a temporary folder whose path is printed at the end.

$ErrorActionPreference = 'Continue'

$repo = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repo

$logDir = Join-Path ([System.IO.Path]::GetTempPath()) ("kahf-build-check-" + [System.Guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $logDir -Force | Out-Null

function Say($text) { Write-Host $text }

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Say 'ERROR: cargo was not found. Install Rust with rustup first (see docs/BUILD.md).'
    exit 2
}

$arch = $env:PROCESSOR_ARCHITECTURE
$rustc = (& rustc --version 2>$null)
Say 'KAHF build check'
Say "Host:      Windows $arch"
Say "Repo:      $repo"
Say "Rust:      $rustc"
Say "Logs:      $logDir"
Say ''

# The kahf-sdr build script links hackrf and rtlsdr from /opt/homebrew, /usr/local
# or /usr/lib only. On Windows those paths do not exist, so the build step
# will fail at link time unless the libraries are found another way.
# cargo check does not link, so it is not affected.
Say 'Note: kahf-sdr links hackrf and rtlsdr using Unix library paths.'
Say '      The cargo build step will fail to link on Windows until that is changed.'
Say ''

$results = @()

function Run-Step($name, [scriptblock]$command, $logName) {
    $log = Join-Path $logDir $logName
    Say "==> $name"
    $output = & $command 2>&1
    $code = $LASTEXITCODE
    $output | Out-File -FilePath $log -Encoding utf8
    if ($code -eq 0) {
        Say '    PASS'
        $script:results += [pscustomobject]@{ Name = $name; Result = 'PASS' }
    } else {
        Say "    FAIL (exit $code). Last lines of $log :"
        $output | Select-Object -Last 15 | ForEach-Object { Say "      $_" }
        $script:results += [pscustomobject]@{ Name = $name; Result = 'FAIL' }
    }
    Say ''
}

Run-Step 'cargo check workspace' { cargo check --workspace } 'check-workspace.log'
Run-Step 'cargo build kahf-cli' { cargo build -p kahf-cli } 'build-kahf-cli.log'

Say 'Summary'
Say '-------'
foreach ($r in $results) {
    Say ('  {0,-6} {1}' -f $r.Result, $r.Name)
}
Say ''
$failed = @($results | Where-Object { $_.Result -ne 'PASS' }).Count
if ($failed -eq 0) {
    Say "Result: all steps passed on Windows $arch."
    $exitCode = 0
} else {
    Say "Result: $failed step(s) failed. Full logs: $logDir"
    $exitCode = 1
}
exit $exitCode
