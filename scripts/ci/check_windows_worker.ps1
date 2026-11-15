# Shared native build/contracts for Checks and hosted Acceptance.
$ErrorActionPreference = 'Stop'
$locator = Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) 'Microsoft Visual Studio/Installer/vswhere.exe'
$installation = & $locator -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -or -not $installation) { throw 'MSVC x64 build tools are unavailable' }
$setup = Join-Path $installation 'Common7/Tools/VsDevCmd.bat'
$command = '"{0}" -no_logo -arch=x64 -host_arch=x64 >nul && set' -f $setup
& cmd.exe /d /c $command | ForEach-Object {
  if ($_ -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
}
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo fmt --manifest-path workers/windows/Cargo.toml -- --check
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo clippy --locked --manifest-path workers/windows/Cargo.toml --all-targets --target-dir target/windows-worker-native -- -D warnings
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo test --locked --manifest-path workers/windows/Cargo.toml --lib --target-dir target/windows-worker-native
if ($LASTEXITCODE) { exit $LASTEXITCODE }
python -m scripts.worker.toolchain --cache target/windows-offline-cache --component windbg_mcp --download
if ($LASTEXITCODE) { exit $LASTEXITCODE }
python -m scripts.worker.build_native
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo fmt --manifest-path workers/package/Cargo.toml -- --check
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo clippy --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks -- -D warnings
if ($LASTEXITCODE) { exit $LASTEXITCODE }
cargo test --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$env:WUKONG_WORKER_CLI = Join-Path $PWD 'target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-worker.exe'
$env:WUKONG_PACKAGE_CLI = Join-Path $PWD 'target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-package.exe'
cargo test --locked --manifest-path workers/package/Cargo.toml --test contract --target-dir target/worker-package-checks -- --ignored
if ($LASTEXITCODE) { exit $LASTEXITCODE }
