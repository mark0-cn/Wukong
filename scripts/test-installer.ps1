$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('re-agent-install-test-' + [Guid]::NewGuid().ToString('N'))
$package = Join-Path $temporary 'package'
$destination = Join-Path $temporary 'installed'
New-Item -ItemType Directory -Path $package -Force | Out-Null
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
& $compiler /nologo /target:exe "/out:$package\re-agent.exe" (Join-Path $PSScriptRoot 'fixtures\installer-client.cs')
if ($LASTEXITCODE -ne 0) { throw 'Could not compile installer process fixture.' }
foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1')) { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $package }
Copy-Item -LiteralPath (Join-Path $root 'docs\user\online-install.md') -Destination (Join-Path $package 'README.md')
$dockerFixture = Join-Path $temporary 'docker-fixture.ps1'
@'
if ($args[0] -eq 'info') { Write-Output 'linux' }
elseif ($args -contains '--format') {
    $reference = [string]$args[2]
    Write-Output ('sha256:' + $reference.Substring($reference.LastIndexOf(':') + 1) + '|linux|amd64')
}
$global:LASTEXITCODE = 0
'@ | Set-Content -LiteralPath $dockerFixture -Encoding utf8
function Set-Package([string]$Identity) {
    $files = @{}
    foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1','README.md','re-agent.exe')) {
        $files[$name] = (Get-FileHash -LiteralPath (Join-Path $package $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    @{ schema_version=2; version='0.1.0'; platform='windows-amd64'; tool_platform='linux/amd64'; image=('hlmark0/re-agent-workbench@' + $Identity); image_id=$Identity; files=$files } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $package 'release.json') -Encoding utf8
}
$old = 'sha256:' + ('a'*64)
$new = 'sha256:' + ('b'*64)
Set-Package $old
& (Join-Path $package 'install-workbench.ps1') -Destination $destination -DockerExecutable $dockerFixture
$config = Join-Path $destination 're-agent.toml'
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
