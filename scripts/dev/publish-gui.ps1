param(
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\RE Agent'),
    [string]$BinaryPath,
    [string]$ConfigPath
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$projectRoot = Get-ReAgentRoot $PSScriptRoot
if (-not $BinaryPath) { $BinaryPath = Join-Path $projectRoot 'target\debug\re-agent.exe' }
if (-not $ConfigPath) { $ConfigPath = Join-Path $projectRoot 'target\debug\re-agent.toml' }
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$config = (Resolve-Path -LiteralPath $ConfigPath).Path
$destinationPath = [IO.Path]::GetFullPath($Destination)
# Build output may inherit the workspace's Low integrity label. Publish to an
# ordinary application directory without changing workspace or Docker ACLs.
if ($destinationPath.Equals($projectRoot, [StringComparison]::OrdinalIgnoreCase) -or
    $destinationPath.StartsWith($projectRoot.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Choose a publication directory outside the source workspace.'
}
New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
$publishedBinary = Join-Path $destinationPath 're-agent.exe'
Copy-Item -LiteralPath $binary -Destination $publishedBinary -Force
$shell = New-Object -ComObject WScript.Shell
$shortcutPath = Join-Path $destinationPath 'RE Agent.lnk'
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $publishedBinary
$shortcut.Arguments = '--config "' + $config + '" gui'
$shortcut.WorkingDirectory = Split-Path -Parent $config
$shortcut.Description = 'RE Agent desktop workspace using the selected configuration'
$shortcut.Save()
Write-Output "Launch: $shortcutPath"
Write-Output "Configuration: $config"
