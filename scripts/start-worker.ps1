param(
    [string]$Container = 're-agent-worker',
    [string]$Image = 're-agent-worker:0.1',
    [switch]$RuntimeOnly,
    [switch]$BuildOnly
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
if ($dockerCommand) {
    $dockerExe = $dockerCommand.Source
} else {
    $dockerExe = Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe'
    if (-not (Test-Path -LiteralPath $dockerExe)) { throw 'Docker CLI not found.' }
}
if ($Container -notmatch '^[A-Za-z0-9][A-Za-z0-9_.-]*$') { throw 'Invalid container name.' }
$buildArgs = @('build', '--file', (Join-Path $projectRoot 'workers\Dockerfile'), '--tag', $Image)
if ($RuntimeOnly) { $buildArgs += @('--build-arg', 'INSTALL_GHIDRA=0') }
& $dockerExe @buildArgs $projectRoot
if ($LASTEXITCODE -ne 0) { throw 'Worker image build failed.' }
if ($BuildOnly) { return }
$existing = & $dockerExe container inspect $Container 2>$null
if ($LASTEXITCODE -eq 0) { throw "Container $Container already exists; inspect it before choosing another name." }
$workerVolume = "$Container-state"
& $dockerExe volume create $workerVolume | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Worker volume creation failed.' }
& $dockerExe run --detach --name $Container --network none --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add SYS_PTRACE --cap-add DAC_OVERRIDE --security-opt no-new-privileges --security-opt seccomp=unconfined --pids-limit 256 --memory 4g --cpus 2 --read-only --tmpfs '/tmp:rw,size=1g' --env HOME=/state/home --env XDG_CACHE_HOME=/state/cache --mount "type=volume,source=$workerVolume,target=/state" $Image
if ($LASTEXITCODE -ne 0) { throw 'Worker container creation failed.' }
Write-Output "Worker ready: $Container. Set runtime.backend='docker' and runtime.docker_container='$Container'."
