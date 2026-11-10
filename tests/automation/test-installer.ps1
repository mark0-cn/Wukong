$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\..\scripts\_lib\repository.ps1')
$root = Get-WukongRoot $PSScriptRoot
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('wukong-install-test-' + [Guid]::NewGuid().ToString('N'))
$package = Join-Path $temporary 'package'
$destination = Join-Path $temporary 'installed'
New-Item -ItemType Directory -Path $package -Force | Out-Null
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
& $compiler /nologo /target:exe "/out:$package\wukong.exe" (Join-Path $PSScriptRoot 'fixtures\installer-client.cs')
if ($LASTEXITCODE -ne 0) { throw 'Could not compile installer process fixture.' }
foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1')) { Copy-Item -LiteralPath (Join-Path $root ('packaging/windows/' + $name)) -Destination $package }
Copy-Item -LiteralPath (Join-Path $root 'docs\user\online-install.md') -Destination (Join-Path $package 'README.md')
$dockerFixture = Join-Path $temporary 'docker-fixture.ps1'
@'
$modeFile = Join-Path $PSScriptRoot 'cache-mode'
$mode = if (Test-Path -LiteralPath $modeFile) { (Get-Content -LiteralPath $modeFile -Raw).Trim() } else { 'digest' }
Add-Content -LiteralPath (Join-Path $PSScriptRoot 'docker-calls.log') -Value ($args -join ' ')
if ($args[0] -eq 'info') { Write-Output 'linux' }
elseif ($args[0] -eq 'pull') { Set-Content -LiteralPath (Join-Path $PSScriptRoot 'pulled') -Value 'yes' }
elseif ($args -contains '--format') {
    $reference = [string]$args[2]
    if (($mode -eq 'id-only' -and $reference.Contains('@')) -or
        ($mode -eq 'missing' -and -not (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'pulled')))) {
        $global:LASTEXITCODE = 1
        return
    }
    $id = if ($mode -eq 'wrong-id') { 'sha256:' + ('c'*64) } else { 'sha256:' + $reference.Substring($reference.LastIndexOf(':') + 1) }
    $platform = if ($mode -eq 'wrong-platform') { 'linux|arm64' } else { 'linux|amd64' }
    Write-Output ($id + '|' + $platform)
}
$global:LASTEXITCODE = 0
'@ | Set-Content -LiteralPath $dockerFixture -Encoding utf8
function Set-Package([string]$Identity) {
    $files = @{}
    foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1','README.md','wukong.exe')) {
        $files[$name] = (Get-FileHash -LiteralPath (Join-Path $package $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    @{ schema_version=2; version='0.1.0'; platform='windows-amd64'; tool_platform='linux/amd64'; image=('hlmark0/wukong@' + $Identity); image_id=$Identity; files=$files } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $package 'release.json') -Encoding utf8
}
$old = 'sha256:' + ('a'*64)
$new = 'sha256:' + ('b'*64)
Set-Package $old
& (Join-Path $package 'install-workbench.ps1') -Destination $destination -DockerExecutable $dockerFixture
$config = Join-Path $destination 'wukong.toml'
Add-Content -LiteralPath $config -Value "`n# preserve-user-settings"
$originalConfig = Get-Content -LiteralPath $config -Raw
Set-Package $new
Set-Content -LiteralPath (Join-Path $destination 'fail-doctor') -Value 'fixture'
$rejected = $false
try { & (Join-Path $package 'install-workbench.ps1') -Destination $destination -DockerExecutable $dockerFixture } catch {
    if ($_ -notmatch 'Tool environment checks failed') { throw }
    $rejected = $true
}
if (-not $rejected -or (Get-Content -LiteralPath (Join-Path $destination 'fixture-image') -Raw) -ne $old) { throw 'Failed upgrade did not restore the previous environment.' }
if ((Get-Content -LiteralPath $config -Raw) -ne $originalConfig) { throw 'Failed upgrade modified user configuration.' }
Remove-Item -LiteralPath (Join-Path $destination 'fail-doctor')
& (Join-Path $package 'install-workbench.ps1') -Destination $destination -DockerExecutable $dockerFixture
if ((Get-Content -LiteralPath (Join-Path $destination 'fixture-image') -Raw) -ne $new) { throw 'Upgrade did not select the new image.' }
& (Join-Path $destination 'rollback-workbench.ps1') -Destination $destination
if ((Get-Content -LiteralPath (Join-Path $destination 'fixture-image') -Raw) -ne $old) { throw 'Manual rollback did not restore the old image.' }
if ((Get-Content -LiteralPath $config -Raw) -ne $originalConfig) { throw 'Manual rollback did not preserve the original configuration.' }
Write-Output 'PASS: install, failed upgrade recovery, successful upgrade and manual rollback; Docker and model APIs were not called.'

# The real installer must skip pull for both registry-digest and config-ID cache hits.
$calls = Join-Path $temporary 'docker-calls.log'
if (Select-String -LiteralPath $calls -Pattern '^pull ') { throw 'Digest cache hit unexpectedly pulled an image.' }
Set-Package $old
Set-Content -LiteralPath (Join-Path $temporary 'cache-mode') -Value 'id-only'
$idDestination = Join-Path $temporary 'installed-by-id'
& (Join-Path $package 'install-workbench.ps1') -Destination $idDestination -DockerExecutable $dockerFixture
if (Select-String -LiteralPath $calls -Pattern '^pull ') { throw 'Config-ID cache hit unexpectedly pulled an image.' }
$idConfig = Get-Content -LiteralPath (Join-Path $idDestination 'wukong.toml') -Raw
if ($idConfig -match 'hlmark0/wukong@' -or $idConfig -notmatch [regex]::Escape($old)) { throw 'ID cache hit did not pass the cached ID to the host CLI.' }
Write-Output 'PASS: cached registry digest and config ID both install without pull.'

Set-Content -LiteralPath (Join-Path $temporary 'cache-mode') -Value 'missing'
& (Join-Path $package 'install-workbench.ps1') -Destination (Join-Path $temporary 'installed-after-pull') -DockerExecutable $dockerFixture
if (@(Select-String -LiteralPath $calls -Pattern '^pull ').Count -ne 1) { throw 'Cache miss should pull exactly once.' }
Write-Output 'PASS: missing image is pulled and verified.'

foreach ($mode in @('wrong-id','wrong-platform')) {
    Set-Content -LiteralPath (Join-Path $temporary 'cache-mode') -Value $mode
    $rejected = $false
    try { & (Join-Path $package 'install-workbench.ps1') -Destination (Join-Path $temporary $mode) -DockerExecutable $dockerFixture } catch {
        if ($_ -notmatch 'Local image identity/platform mismatch') { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw "Cache mismatch was accepted: $mode" }
}
if (@(Select-String -LiteralPath $calls -Pattern '^pull ').Count -ne 1) { throw 'Mismatched image triggered an unexpected pull.' }
Write-Output 'PASS: wrong cached image identity or architecture is rejected.'
