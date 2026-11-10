param([switch]$Force)
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$arguments = @('-m', 'scripts.toolchain.fetch_worker_deps')
if ($Force) { $arguments += '--force' }
Push-Location -LiteralPath (Get-ReAgentRoot $PSScriptRoot)
try {
    & python @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Worker dependency download failed.' }
} finally { Pop-Location }
