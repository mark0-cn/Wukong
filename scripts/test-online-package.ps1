$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('re-agent-package-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1')) { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $temporary }
Copy-Item -LiteralPath (Join-Path $root 'docs/online-install.md') -Destination (Join-Path $temporary 'README.md')
Set-Content -LiteralPath (Join-Path $temporary 're-agent.exe') -Value 'checksum fixture; never executed'
$files = @{}
foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1','README.md','re-agent.exe')) { $files[$name] = (Get-FileHash -LiteralPath (Join-Path $temporary $name) -Algorithm SHA256).Hash.ToLowerInvariant() }
$manifest = @{ schema_version=2; platform='windows-amd64'; tool_platform='linux/amd64'; image=('hlmark0/re-agent-workbench@sha256:' + ('a'*64)); image_id=('sha256:' + ('b'*64)); files=$files }
$manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $temporary 'release.json') -Encoding utf8
& (Join-Path $temporary 'install-workbench.ps1') -ValidateOnly
Add-Content -LiteralPath (Join-Path $temporary 're-agent.exe') -Value 'tampered'
$rejected = $false
try { & (Join-Path $temporary 'install-workbench.ps1') -ValidateOnly } catch {
    if ($_ -notmatch 'Checksum mismatch') { throw }
    $rejected = $true
}
if (-not $rejected) { throw 'Tampered client was accepted.' }
Write-Output 'PASS: online package validation accepts matching files and rejects tampering.'
