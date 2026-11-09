param(
    [Parameter(Mandatory=$true)][string]$Destination,
    [Parameter(Mandatory=$true)][string]$ImageMetadata,
    [Parameter(Mandatory=$true)][string]$Tag,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '..\target\release\re-agent.exe')
)
$ErrorActionPreference = 'Stop'
& python (Join-Path $PSScriptRoot 'release.py') package --tag $Tag --binary $BinaryPath --image-metadata $ImageMetadata --output $Destination
if ($LASTEXITCODE -ne 0) { throw 'Online package creation failed.' }
& python (Join-Path $PSScriptRoot 'release.py') checksums $Destination
if ($LASTEXITCODE -ne 0) { throw 'Checksum generation failed.' }
