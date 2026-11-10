param(
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\RE Agent Workbench'),
    [string]$DockerExecutable = 'docker',
    [switch]$ValidateOnly
)
$ErrorActionPreference = 'Stop'
$release = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'release.json') -Raw | ConvertFrom-Json
if ($release.schema_version -ne 2 -or $release.platform -ne 'windows-amd64' -or $release.tool_platform -ne 'linux/amd64') { throw 'Unsupported online package.' }
if ($release.image -notmatch '^[a-z0-9][a-z0-9_-]*/re-agent-workbench@sha256:[0-9a-f]{64}$' -or $release.image_id -notmatch '^sha256:[0-9a-f]{64}$') { throw 'Package must pin a Docker Hub image digest and image ID.' }
$members = @('re-agent.exe','install-workbench.ps1','rollback-workbench.ps1','README.md')
foreach ($required in $members) {
    if (-not $release.files.PSObject.Properties[$required]) { throw "Missing package checksum: $required" }
}
foreach ($entry in $release.files.PSObject.Properties) {
    if ($entry.Name -notin $members) { throw 'Invalid package member.' }
    $hash = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $entry.Name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $entry.Value) { throw "Checksum mismatch: $($entry.Name)" }
}
if ($ValidateOnly) { Write-Output 'Online package checksums and image identity are valid.'; return }
if ($env:OS -ne 'Windows_NT' -or [Environment]::Is64BitOperatingSystem -ne $true -or $env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { throw 'This installer requires Windows x64.' }
if (-not (Get-Command $DockerExecutable -ErrorAction SilentlyContinue)) {
    $desktopDocker = Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe'
    if (Test-Path -LiteralPath $desktopDocker) { $DockerExecutable = $desktopDocker }
    else { throw 'Install and start Docker Desktop with Linux containers, then run this installer again.' }
}
$engine = & $DockerExecutable info --format '{{.OSType}}'
if ($LASTEXITCODE -ne 0 -or ($engine -join '').Trim() -ne 'linux') { throw 'Start Docker Desktop and switch to Linux containers, then retry.' }
Write-Host "Preparing tool image for version $($release.version)..."
& $DockerExecutable image inspect $release.image *> $null
if ($LASTEXITCODE -ne 0) {
    $pulled = $false
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        & $DockerExecutable pull --platform linux/amd64 $release.image
        if ($LASTEXITCODE -eq 0) { $pulled = $true; break }
        if ($attempt -lt 3) { Start-Sleep -Seconds 2 }
    }
    if (-not $pulled) { throw 'Image pull failed. For this private repository, run: docker login --username hlmark0 (or another authorized account). Also check network and Docker disk space, then retry. Do not use the CI write token on client machines.' }
}
$actual = & $DockerExecutable image inspect $release.image --format '{{.Id}}|{{.Os}}|{{.Architecture}}'
if ($LASTEXITCODE -ne 0 -or ($actual -join '').Trim() -ne "$($release.image_id)|linux|amd64") { throw 'Downloaded image identity/platform mismatch.' }
$destinationPath = [IO.Path]::GetFullPath($Destination)
New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
$exe = Join-Path $destinationPath 're-agent.exe'
if ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot 're-agent.exe')) -eq $exe) { throw 'Extract the new package into a separate folder before installing.' }
if (Get-Process -Name re-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) { throw 'Close the installed RE Agent client before updating.' }
$drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($destinationPath))
if ($drive.AvailableFreeSpace -lt 1GB) { throw 'At least 1 GiB free space is required on the installation drive. Docker image storage needs additional space.' }
$config = Join-Path $destinationPath 're-agent.toml'
# Publish the candidate into the normal application directory before invoking it.
# A source/extraction folder can carry a Low integrity label that denies Docker pipe access.
$stage = Join-Path $destinationPath ('.staging\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -Force | Out-Null
$newExe = Join-Path $stage 're-agent.exe'
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 're-agent.exe') -Destination $newExe
function Invoke-WorkbenchJson([string[]]$Arguments) {
    $result = & $newExe --config $config @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Workbench command failed: $($Arguments -join ' ')" }
    return (($result -join "`n") | ConvertFrom-Json)
}
$previousImage = $null
$backup = $null
$switched = $false
Push-Location -LiteralPath $destinationPath
try {
    if (Test-Path -LiteralPath $config) {
        $status = Invoke-WorkbenchJson -Arguments @('env','status')
        if ($status.environment) { $previousImage = $status.environment.image_id }
        $backup = Join-Path $destinationPath ('.previous\' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $backup -Force | Out-Null
        foreach ($name in @('re-agent.exe','re-agent.toml','release.json','README.md','install-workbench.ps1','rollback-workbench.ps1')) {
            $source = Join-Path $destinationPath $name
            if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $backup }
        }
    } else {
        $docker = $DockerExecutable | ConvertTo-Json -Compress
        $image = $release.image | ConvertTo-Json -Compress
        @"
state_dir = "data/assessments"
[runtime]
docker_executable = $docker
[workbench]
root = "data/workbench"
image = $image
"@ | Set-Content -LiteralPath $config -Encoding utf8
    }
    if ($previousImage -and $previousImage -ne $release.image_id) {
        $up = Invoke-WorkbenchJson -Arguments @('env','update','--image',$release.image)
    } else {
        $up = Invoke-WorkbenchJson -Arguments @('env','up','--image',$release.image)
    }
    $switched = $true
    $doctor = Invoke-WorkbenchJson -Arguments @('env','doctor')
    if (-not $doctor.passed) { throw ('Tool environment checks failed: ' + ($doctor | ConvertTo-Json -Depth 8 -Compress)) }
    Copy-Item -LiteralPath $up.config -Destination $config -Force
    foreach ($name in $members + @('release.json')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $destinationPath -Force
    }
    if ($backup -and $previousImage -and (Test-Path -LiteralPath (Join-Path $backup 're-agent.exe'))) {
        @{ schema_version=1; directory=$backup; image_id=$previousImage } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $destinationPath 'rollback.json') -Encoding utf8
    }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut((Join-Path $destinationPath 'RE Agent Workbench.lnk'))
    $shortcut.TargetPath = $exe
    $shortcut.Arguments = '--config "' + $config + '" gui'
    $shortcut.WorkingDirectory = $destinationPath
    $shortcut.Save()
    Write-Output "Installed version $($release.version): $destinationPath"
    Write-Output 'Tool checks passed. Open RE Agent Workbench.lnk and configure your model credentials.'
} catch {
    $originalError = $_
    if ($switched -and $previousImage) {
        try { $null = Invoke-WorkbenchJson -Arguments @('env','update','--image',$previousImage) }
        catch { Write-Warning "Environment restore failed: $_. Backup is at $backup" }
    } elseif ($switched) {
        try { $null = Invoke-WorkbenchJson -Arguments @('env','down') } catch { Write-Warning "Could not stop incomplete installation: $_" }
    }
    if ($backup) {
        foreach ($file in Get-ChildItem -LiteralPath $backup -File) { Copy-Item -LiteralPath $file.FullName -Destination $destinationPath -Force }
    }
    throw $originalError
} finally {
    Pop-Location
    Remove-Item -LiteralPath $newExe -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $stage -ErrorAction SilentlyContinue
}
