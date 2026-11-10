$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib\repository.ps1')
$root = Get-WukongRoot $PSScriptRoot
$generated = Join-Path $root 'target\generated-icons'
Push-Location -LiteralPath $root
try {
    & npx --no-install tauri icon icons/icon.svg --output $generated
    if ($LASTEXITCODE -ne 0) { throw 'Icon generation failed.' }
    # Only desktop assets are distributed by this project.
    foreach ($name in @('icon.png', 'icon.ico', 'icon.icns', '32x32.png', '64x64.png',
        '128x128.png', '128x128@2x.png', 'StoreLogo.png', 'Square30x30Logo.png',
        'Square44x44Logo.png', 'Square71x71Logo.png', 'Square89x89Logo.png',
        'Square107x107Logo.png', 'Square142x142Logo.png', 'Square150x150Logo.png',
        'Square284x284Logo.png', 'Square310x310Logo.png')) {
        Copy-Item -LiteralPath (Join-Path $generated $name) -Destination (Join-Path $root "icons/$name")
    }
} finally { Pop-Location }
