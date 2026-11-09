param([switch]$Force)
# Fetch and checksum-verify the large worker-image dependencies once, on the
# host, so docker builds never re-download them (and never fail on network
# flakiness). Re-run with -Force to refresh.
$deps = Join-Path (Split-Path -Parent $PSScriptRoot) 'workers\deps'
New-Item -ItemType Directory -Force $deps | Out-Null

$ghidraUrl = 'https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.2_build/ghidra_12.2_PUBLIC_20261016.zip'
$ghidraSha = '2c2cd776db3c1f6a097ba418922378ac3649cadd5e844a6f5f088ae628a7be67'
$temurinUrl = 'https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5%2B8/OpenJDK25U-jdk_x64_linux_hotspot_25.0.5_8.tar.gz'
$temurinSha = '68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb'
$gefUrl = 'https://raw.githubusercontent.com/hugsy/gef/2024.06/gef.py'
$gefSha = '12881b1d42fb490ca72896099daf5c128c7c7382be0779c8f8f7db3a9161ab6f'

function Get-Verified([string]$url, [string]$sha, [string]$name) {
    $path = Join-Path $deps $name
    if ((Test-Path $path) -and -not $Force) {
        $actual = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLower()
        if ($actual -eq $sha) { Write-Host "$name cached and verified"; return $path }
        Write-Host "$name present but checksum mismatch; re-downloading"
    }
    Write-Host "downloading $name ..."
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if (-not $curl) { throw 'curl.exe is required (ships with Windows 10+)' }
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        # curl progress goes to stderr; with ErrorActionPreference=Stop that
        # would terminate the script on Windows PowerShell 5.1. Check the
        # exit code instead.
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        & curl.exe --fail --location --retry 8 --retry-all-errors -C - -o $path $url 2>&1 | Out-Null
        $code = $LASTEXITCODE
        $ErrorActionPreference = $previous
        if ($code -eq 0) { break }
        Write-Host "attempt $attempt failed (exit $code); retrying..."
        Start-Sleep -Seconds 3
    }
    if ($code -ne 0) { throw "$name download failed after retries" }
    $actual = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLower()
    if ($actual -ne $sha) {
        Remove-Item $path -ErrorAction SilentlyContinue
        throw "$name checksum mismatch: expected $sha, got $actual"
    }
    Write-Host "$name downloaded and verified"
    return $path
}

Get-Verified $ghidraUrl $ghidraSha 'ghidra.zip' | Out-Null
Get-Verified $temurinUrl $temurinSha 'temurin25.tar.gz' | Out-Null
Get-Verified $gefUrl $gefSha 'gef.py' | Out-Null
Write-Host "worker dependencies ready in $deps"
