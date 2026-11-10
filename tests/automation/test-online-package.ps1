$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\..\scripts\_lib\repository.ps1')
$root = Get-ReAgentRoot $PSScriptRoot
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('re-agent-package-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
foreach ($name in @('install-workbench.ps1','rollback-workbench.ps1')) { Copy-Item -LiteralPath (Join-Path $root ('packaging/windows/' + $name)) -Destination $temporary }
Copy-Item -LiteralPath (Join-Path $root 'docs/user/online-install.md') -Destination (Join-Path $temporary 'README.md')
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

# Exercise the relocated packaging wrapper from a different PowerShell location.
Push-Location -LiteralPath $root
try {
    $version = & python -m scripts.release.cli check
    if ($LASTEXITCODE) { throw 'Version check failed.' }
    $revision = & git rev-parse HEAD
    if ($LASTEXITCODE) { throw 'Source revision check failed.' }
} finally { Pop-Location }
$metadata = @{ repository='hlmark0/re-agent-workbench'; digest=('sha256:' + ('a'*64)); image_id=('sha256:' + ('b'*64)); platform='linux/amd64'; source_revision=$revision; size_bytes=123 }
$metadata | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $temporary 'image.json') -Encoding utf8
Push-Location -LiteralPath $temporary
try {
    & (Join-Path $root 'scripts/release/package-workbench.ps1') -Destination './output with spaces' -ImageMetadata './image.json' -BinaryPath './re-agent.exe' -Tag "v$version"
    if ((Get-Location).Path -ne $temporary) { throw 'Packaging changed the caller location.' }
    $output = Join-Path $temporary 'output with spaces'
    $archive = Join-Path $output "re-agent-v$version-windows-amd64.zip"
    Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $output 'expanded')
    & (Join-Path $output 'expanded/install-workbench.ps1') -ValidateOnly
    if (-not (Test-Path -LiteralPath (Join-Path $output 'SHA256SUMS'))) { throw 'Packaging omitted checksums.' }
} finally { Pop-Location }
Write-Output 'PASS: packaging resolves caller-relative paths and validates the relocated installer payload.'
