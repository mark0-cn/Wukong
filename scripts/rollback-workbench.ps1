param([string]$Destination = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
$destinationPath = [IO.Path]::GetFullPath($Destination)
$record = Get-Content -LiteralPath (Join-Path $destinationPath 'rollback.json') -Raw | ConvertFrom-Json
$backup = (Resolve-Path -LiteralPath $record.directory).Path
$allowed = Join-Path $destinationPath '.previous'
if (-not $backup.StartsWith($allowed + '\', [StringComparison]::OrdinalIgnoreCase) -or $record.image_id -notmatch '^sha256:[0-9a-f]{64}$') { throw 'Invalid rollback record.' }
$exe = Join-Path $destinationPath 're-agent.exe'
if (Get-Process -Name re-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) { throw 'Close RE Agent before rollback.' }
$config = Join-Path $destinationPath 're-agent.toml'
if (-not (Test-Path -LiteralPath (Join-Path $backup 're-agent.exe'))) { throw 'Previous client is missing.' }
Push-Location -LiteralPath $destinationPath
try {
    & $exe --config $config env update --image $record.image_id
    if ($LASTEXITCODE -ne 0) { throw 'Environment rollback failed; client was not changed.' }
    $doctor = & $exe --config $config env doctor
    if ($LASTEXITCODE -ne 0 -or -not (($doctor -join "`n" | ConvertFrom-Json).passed)) { throw 'Previous environment did not pass doctor; inspect before proceeding.' }
    foreach ($name in @('re-agent.exe','re-agent.toml','release.json','README.md','install-workbench.ps1','rollback-workbench.ps1')) {
        $source = Join-Path $backup $name
        if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $destinationPath -Force }
    }
    Write-Output 'Previous client, configuration and tool image restored. Challenge data was preserved.'
} finally { Pop-Location }
