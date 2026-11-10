param(
    [Parameter(Mandatory=$true)][string]$Destination,
    [Parameter(Mandatory=$true)][string]$ImageMetadata,
    [Parameter(Mandatory=$true)][string]$Tag,
    [string]$BinaryPath = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$root = Get-WukongRoot $PSScriptRoot
if (-not $BinaryPath) { $BinaryPath = Join-Path $root 'target/release/wukong.exe' }
# Resolve user paths before changing cwd for Python module execution.
$BinaryPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($BinaryPath)
$ImageMetadata = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($ImageMetadata)
$Destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)
Push-Location -LiteralPath $root
try {
    & python -m scripts.release.cli package --tag $Tag --binary $BinaryPath --image-metadata $ImageMetadata --output $Destination
    if ($LASTEXITCODE -ne 0) { throw 'Online package creation failed.' }
    & python -m scripts.release.cli checksums $Destination
    if ($LASTEXITCODE -ne 0) { throw 'Checksum generation failed.' }
} finally { Pop-Location }
