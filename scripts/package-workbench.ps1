param(
    [Parameter(Mandatory=$true)][string]$Destination,
    [string]$BinaryPath = (Join-Path $PSScriptRoot '..\target\debug\re-agent.exe'),
    [string]$DockerExecutable = 'docker',
    [string]$Image = 're-agent-workbench:0.1',
    [switch]$IncludeImage
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $output) { throw 'Choose a new package directory.' }
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$imageId = & $DockerExecutable image inspect $Image --format '{{.Id}}'
if ($LASTEXITCODE -ne 0) { throw 'Tool image not available.' }
if ($IncludeImage -and $output -match '^[A-Za-z]:\\') {
    $size = [long](& $DockerExecutable image inspect $imageId.Trim() --format '{{.Size}}')
    if ($LASTEXITCODE -ne 0) { throw 'Cannot estimate image size.' }
    $free = (Get-PSDrive -Name $output.Substring(0,1)).Free
    if ($free -lt ($size * 2.5 + 512MB)) { throw 'Choose a drive with more free space for image export and temporary data.' }
}
New-Item -ItemType Directory -Path $output | Out-Null
Copy-Item -LiteralPath $binary -Destination (Join-Path $output 're-agent.exe')
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install-workbench.ps1') -Destination $output
Copy-Item -LiteralPath (Join-Path $root 'docs\pwn-workbench.md') -Destination (Join-Path $output 'README.md')
$files = @{}
foreach ($name in @('re-agent.exe','install-workbench.ps1','README.md')) { $files[$name] = (Get-FileHash -LiteralPath (Join-Path $output $name) -Algorithm SHA256).Hash.ToLowerInvariant() }
if ($IncludeImage) {
    & $DockerExecutable image save -o (Join-Path $output 'tool-image.tar') $imageId.Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Image export failed.' }
    $files['tool-image.tar'] = (Get-FileHash -LiteralPath (Join-Path $output 'tool-image.tar') -Algorithm SHA256).Hash.ToLowerInvariant()
}
@{ schema_version=1; platform='windows-amd64'; tool_platform='linux/amd64'; image=$Image; image_id=$imageId.Trim(); files=$files } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $output 'release.json') -Encoding utf8
Write-Output "Package: $output"
