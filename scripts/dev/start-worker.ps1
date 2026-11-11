param(
    [string]$Container = 'wukong-worker',
    [string]$Image = 'wukong-worker:0.1',
    [switch]$RuntimeOnly,
    [switch]$BuildOnly
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$projectRoot = Get-WukongRoot $PSScriptRoot
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
# PS 5.1 promotes native stderr lines to terminating errors while
# ErrorActionPreference is 'Stop'; the inspect probe below expects a
# non-zero exit, not a thrown error.
$previous = $ErrorActionPreference
$ErrorActionPreference = 'SilentlyContinue'
$existing = & $dockerExe container inspect $Container 2>$null
$ErrorActionPreference = $previous
if ($LASTEXITCODE -eq 0) { throw "Container $Container already exists; inspect it before choosing another name." }
$workerVolume = "$Container-state"
& $dockerExe volume create $workerVolume | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Worker volume creation failed.' }
& $dockerExe run --detach --name $Container --network none --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL --cap-add SYS_PTRACE --cap-add DAC_OVERRIDE --security-opt no-new-privileges --security-opt seccomp=unconfined --pids-limit 256 --memory 4g --cpus 2 --read-only --tmpfs '/tmp:rw,size=1g,exec' --env HOME=/state/home --env XDG_CACHE_HOME=/state/cache --mount "type=volume,source=$workerVolume,target=/state" $Image
if ($LASTEXITCODE -ne 0) { throw 'Worker container creation failed.' }
# A fresh volume copies the image's /state ownership (worker:worker, see the
# Dockerfile); the bridge daemon runs as root and must chmod the state root
# itself, which needs root ownership (the minimal capability set drops
# CAP_FOWNER). Do the one-time correction here.
& $dockerExe exec $Container chown 0:0 /state 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Worker state ownership fix failed.' }
Write-Output "Worker ready: $Container. Register environments.<id>.runtime.backend='docker' and environments.<id>.runtime.docker_container='$Container', then select default_environment='<id>'."
