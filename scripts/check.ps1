param([switch]$Release, [string]$DockerContainer)
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot -ErrorAction Stop
try {
    # Do not set ErrorActionPreference='Stop' globally: on Windows PowerShell
    # 5.1, redirecting native stderr (2>&1) while it is 'Stop' promotes
    # ordinary stderr lines to terminating errors. Rely on $LASTEXITCODE
    # checks instead; each failing step throws an explicit message.
    & cargo fmt --all -- --check 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Rust formatting check failed.' }
    & cargo clippy --all-targets -- -D warnings 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Rust lint check failed.' }
    & cargo test --all-targets 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
    & python -m compileall -q workers/runtime 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Worker module syntax check failed.' }
    & python -m py_compile fixtures/evaluate.py fixtures/score_reports.py fixtures/analysis/evaluate.py fixtures/analysis/score.py 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Python syntax check failed.' }
    & python workers/runtime/test_bridge.py 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Worker integration tests failed.' }
    & python fixtures/analysis/test_analysis.py 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'General analysis scoring tests failed.' }
    if ($DockerContainer) {
        $dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
        $dockerExe = if ($dockerCommand) { $dockerCommand.Source } else { Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe' }
        $testState = '/state/acceptance-' + [Guid]::NewGuid().ToString('N')
        & $dockerExe exec $DockerContainer python3 /opt/re-agent/runtime/integration_test.py --state $testState --fuzz 2>&1
        if ($LASTEXITCODE -ne 0) { throw 'Linux worker acceptance failed.' }
    }
    if ($Release) {
        & cargo build --release 2>&1
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    }
} finally {
    Pop-Location
}
