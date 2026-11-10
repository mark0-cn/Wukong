param(
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\Wukong'),
    [string]$BinaryPath,
    [string]$ConfigPath
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$projectRoot = Get-WukongRoot $PSScriptRoot
if (-not $BinaryPath) { $BinaryPath = Join-Path $projectRoot 'target\debug\wukong.exe' }
if (-not $ConfigPath) { $ConfigPath = Join-Path $projectRoot 'target\debug\wukong.toml' }
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
$publishedBinary = Join-Path $destinationPath 'wukong.exe'
Copy-Item -LiteralPath $binary -Destination $publishedBinary -Force
$shell = New-Object -ComObject WScript.Shell
$shortcutPath = Join-Path $destinationPath 'Wukong.lnk'
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $publishedBinary
$shortcut.Arguments = '--config "' + $config + '" gui'
$shortcut.WorkingDirectory = Split-Path -Parent $config
$shortcut.Description = 'Wukong desktop workspace using the selected configuration'
$shortcut.Save()
Write-Output "Launch: $shortcutPath"
Write-Output "Configuration: $config"
