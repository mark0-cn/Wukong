param(
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\RE Agent Workbench'),
    [string]$DockerExecutable = 'docker'
)
$ErrorActionPreference = 'Stop'
$release = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'release.json') -Raw | ConvertFrom-Json
if ($release.schema_version -ne 1 -or $release.platform -ne 'windows-amd64') { throw 'Unsupported package.' }
foreach ($required in @('re-agent.exe','install-workbench.ps1','README.md')) {
    if (-not $release.files.PSObject.Properties[$required]) { throw "Missing package checksum: $required" }
}
if ((Test-Path -LiteralPath (Join-Path $PSScriptRoot 'tool-image.tar')) -and -not $release.files.PSObject.Properties['tool-image.tar']) { throw 'Unverified image archive.' }
foreach ($entry in $release.files.PSObject.Properties) {
    if ($entry.Name -notin @('re-agent.exe','install-workbench.ps1','README.md','tool-image.tar')) { throw 'Invalid package member.' }
    $hash = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $entry.Name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $entry.Value) { throw "Checksum mismatch: $($entry.Name)" }
}
$destinationPath = [IO.Path]::GetFullPath($Destination)
New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
$exe = Join-Path $destinationPath 're-agent.exe'
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 're-agent.exe') -Destination $exe -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'README.md') -Destination $destinationPath -Force
if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'tool-image.tar')) {
    & $DockerExecutable image load -i (Join-Path $PSScriptRoot 'tool-image.tar')
    if ($LASTEXITCODE -ne 0) { throw 'Image load failed.' }
}
& $DockerExecutable image inspect $release.image_id *> $null
if ($LASTEXITCODE -ne 0) { throw 'Load the matching tool image first, or use the package containing tool-image.tar.' }
$config = Join-Path $destinationPath 're-agent.toml'
if (-not (Test-Path -LiteralPath $config)) {
    $docker = $DockerExecutable | ConvertTo-Json -Compress
    $image = $release.image_id | ConvertTo-Json -Compress
    @"
state_dir = "data/assessments"
[runtime]
docker_executable = $docker
[workbench]
root = "data/workbench"
image = $image
"@ | Set-Content -LiteralPath $config -Encoding utf8
}
$up = & $exe --config $config env up
if ($LASTEXITCODE -ne 0) { throw 'Environment startup failed.' }
$up | Write-Output
$generated = (($up -join "`n") | ConvertFrom-Json).config
if (-not (Test-Path -LiteralPath $generated)) { throw 'Generated runtime configuration is missing.' }
Copy-Item -LiteralPath $generated -Destination $config -Force
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path $destinationPath 'RE Agent Workbench.lnk'))
$shortcut.TargetPath=$exe
$shortcut.Arguments='--config "'+$config+'" gui'
$shortcut.WorkingDirectory=$destinationPath
$shortcut.Save()
Write-Output "Installed: $destinationPath"
Write-Output "Configure model credentials on this machine, then open RE Agent Workbench.lnk."
