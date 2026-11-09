param([switch]$Force)
$arguments = @((Join-Path $PSScriptRoot "fetch-worker-deps.py"))
if ($Force) { $arguments += "--force" }
& python @arguments
if ($LASTEXITCODE -ne 0) { throw "Worker dependency download failed." }
