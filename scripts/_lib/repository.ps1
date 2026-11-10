function Get-ReAgentRoot([string]$StartPath) {
    $candidate = [IO.Path]::GetFullPath($StartPath)
    while ($candidate) {
        if ((Test-Path -LiteralPath (Join-Path $candidate 'Cargo.toml')) -and
            (Test-Path -LiteralPath (Join-Path $candidate 'rust-toolchain.toml'))) { return $candidate }
        $candidate = Split-Path -Parent $candidate
    }
    throw 'Cannot locate re-agent repository root.'
}
