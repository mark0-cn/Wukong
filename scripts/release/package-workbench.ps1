param(
    [Parameter(Mandatory=$true)][string]$Destination,
    [Parameter(Mandatory=$true)][string]$ImageMetadata,
    [Parameter(Mandatory=$true)][string]$Tag,
    [string]$BinaryPath = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$root = Get-ReAgentRoot $PSScriptRoot
if (-not $BinaryPath) { $BinaryPath = Join-Path $root 'target/release/re-agent.exe' }
# Resolve user paths before changing cwd for Python module execution.
$BinaryPath = [IO.Path]::GetFullPath($BinaryPath)
$ImageMetadata = [IO.Path]::GetFullPath($ImageMetadata)
$Destination = [IO.Path]::GetFullPath($Destination)
Push-Location -LiteralPath $root
try {
    & python -m scripts.release.cli package --tag $Tag --binary $BinaryPath --image-metadata $ImageMetadata --output $Destination
    if ($LASTEXITCODE -ne 0) { throw 'Online package creation failed.' }
    & python -m scripts.release.cli checksums $Destination
    if ($LASTEXITCODE -ne 0) { throw 'Checksum generation failed.' }
} finally { Pop-Location }
