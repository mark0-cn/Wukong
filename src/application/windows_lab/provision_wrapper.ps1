# One-shot offline worker provisioning wrapper injected into a powered-off
# Windows guest and launched by a local Group Policy machine startup script.
# This is a fixed controller asset: identities come from provision-plan.json
# beside it and the receipt is written beside the plan. The startup entry must
# run exactly once: the finally block restores the previous machine policy files,
# deletes the startup launcher, schedules removal of this script and plan, then
# always powers the guest off.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSCommandPath
$planPath = Join-Path $root 'provision-plan.json'
$receiptPath = Join-Path $root 'receipt.json'
$existingReceipt = Test-Path -LiteralPath $receiptPath
$receipt = [ordered]@{
    schema = 2
    provision_id = $null
    installed = $false
    already_installed = $false
    exit_code = $null
    connection = $null
    startup_cleanup = 'not_attempted'
    startup_script_deleted = $false
    cleanup_scheduled = $false
    error = $null
}
function Save-Receipt {
    try {
        $json = $receipt | ConvertTo-Json -Depth 8
        $temporary = $receiptPath + '.tmp'
        [System.IO.File]::WriteAllText($temporary, $json, (New-Object System.Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $temporary -Destination $receiptPath -Force
    } catch {
    }
}
# Restore the bytes we backed up and delete the per-operation backup file.
function Restore-PolicyFile([string]$path, [string]$backup) {
    if (-not $path) {
        return 'skipped'
    }
    if ($backup -and (Test-Path -LiteralPath $backup)) {
        Copy-Item -LiteralPath $backup -Destination $path -Force
        Remove-Item -LiteralPath $backup -Force
        return 'restored'
    }
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
    return 'removed'
}
# Fallback used only when the plan cannot be read: remove our startup command
# lines from scripts.ini and delete the document if nothing else remains.
function Remove-StartupEntry([string]$scriptsIni, [string]$entry) {
    if (-not (Test-Path -LiteralPath $scriptsIni)) {
        return 'removed'
    }
    $lines = @(Get-Content -LiteralPath $scriptsIni)
    $kept = New-Object System.Collections.Generic.List[string]
    $index = $null
    foreach ($line in $lines) {
        if ($line -match '^(\d+)CmdLine=(.*)$' -and $Matches[2].Trim() -eq $entry) {
            $index = $Matches[1]
            continue
        }
        $kept.Add($line)
    }
    if ($null -ne $index) {
        $kept = @($kept | Where-Object { $_.Trim() -ne ($index + 'Parameters=') })
    }
    $content = @($kept | Where-Object { $_.Trim() -ne '' })
    if ($content.Count -eq 0) {
        Remove-Item -LiteralPath $scriptsIni -Force
        return 'removed'
    }
    if ($null -ne $index) {
        Set-Content -LiteralPath $scriptsIni -Value $kept -Encoding Unicode
        return 'removed'
    }
    return 'removed'
}
$plan = $null
try {
    $plan = Get-Content -LiteralPath $planPath -Raw -ErrorAction Stop | ConvertFrom-Json
    $receipt.provision_id = [string]$plan.provision_id
    if ($existingReceipt) {
        $receipt.already_installed = $true
    } else {
        $mediaRoot = $null
        foreach ($disk in @(Get-CimInstance -ClassName Win32_LogicalDisk)) {
            if ([string]$disk.VolumeName -eq [string]$plan.media_label -and [int]$disk.DriveType -eq 5) {
                $mediaRoot = [string]$disk.DeviceID + '\'
                break
            }
        }
        if (-not $mediaRoot) {
            throw 'offline media volume WUKONG_OFFLINE is not attached'
        }
        $setup = Join-Path $mediaRoot 'WukongWorkerSetup.exe'
        if ((Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant() -ne [string]$plan.setup_sha256) {
            throw 'offline media setup executable hash mismatch'
        }
        if ((Get-FileHash -LiteralPath (Join-Path $mediaRoot 'enrollment\enrollment.json') -Algorithm SHA256).Hash.ToLowerInvariant() -ne [string]$plan.enrollment_sha256) {
            throw 'offline media enrollment hash mismatch'
        }
        if ((Get-FileHash -LiteralPath (Join-Path $mediaRoot 'payload\manifest.json') -Algorithm SHA256).Hash.ToLowerInvariant() -ne [string]$plan.manifest_sha256) {
            throw 'offline media payload hash mismatch'
        }
        $verifyText = (& $setup verify-enrollment (Join-Path $mediaRoot 'payload') (Join-Path $mediaRoot 'enrollment') ([string]$plan.controller_public_key) ([string]$plan.controller_version) 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) {
            throw ('offline media verification failed: ' + $verifyText.Trim())
        }
        $verified = $verifyText | ConvertFrom-Json
        if ($verified.verified -ne $true) {
            throw 'offline media verification did not pass'
        }
        if ([string]$verified.installation_id -ne [string]$plan.installation_id) {
            throw 'offline media installation identity mismatch'
        }
        if ([string]$verified.manifest_sha256 -ne [string]$plan.manifest_sha256) {
            throw 'offline media payload identity mismatch'
        }
        if ([string]$verified.execution_boundary -ne 'disposable_vm') {
            throw 'offline media execution boundary is unsupported'
        }
        $installText = (& $setup install-reviewed (Join-Path $mediaRoot 'setup.json') ([string]$plan.controller_public_key) ([string]$plan.enrollment_sha256))
        $exitCode = $LASTEXITCODE
        $receipt.exit_code = $exitCode
        $installText = @($installText) -join "`n"
        if ($installText.Length -gt 65536) {
            throw 'worker installation output exceeded the controller budget'
        }
        if ($exitCode -ne 0) {
            throw ('worker installation failed with exit code ' + $exitCode)
        }
        try {
            $connection = $installText | ConvertFrom-Json
        } catch {
            throw 'worker installation did not return a JSON connection receipt'
        }
        if ([int]$connection.schema -ne 1) {
            throw 'worker connection receipt schema mismatch'
        }
        if ($connection.installed -ne $true) {
            throw 'worker installation did not report an installed worker'
        }
        if ([string]$connection.installation_id -ne [string]$plan.installation_id) {
            throw 'worker connection installation identity mismatch'
        }
        if ([string]$connection.environment_id -ne [string]$plan.environment_id) {
            throw 'worker connection environment identity mismatch'
        }
        if ([string]$connection.manifest_sha256 -ne [string]$plan.manifest_sha256) {
            throw 'worker connection payload identity mismatch'
        }
        if ([int]$connection.ssh_port -ne [int]$plan.ssh_port) {
            throw 'worker connection SSH port mismatch'
        }
        $receipt.connection = $connection
        $receipt.installed = $true
    }
} catch {
    $receipt.error = [string]$_.Exception.Message
} finally {
    # Record the installation outcome before any cleanup so a crash below still
    # leaves controller-readable evidence.
    if (-not $existingReceipt) {
        Save-Receipt
    }
    $startupScript = $null
    $scriptsIni = $null
    $scriptsIniBackup = $null
    $gptIni = $null
    $gptIniBackup = $null
    $startupEntry = $null
    if ($plan) {
        $startupScript = [string]$plan.startup_script
        $scriptsIni = [string]$plan.scripts_ini
        $scriptsIniBackup = [string]$plan.scripts_ini_backup
        $gptIni = [string]$plan.gpt_ini
        $gptIniBackup = [string]$plan.gpt_ini_backup
    }
    $provisionId = Split-Path -Leaf $root
    if ($provisionId -match '^[0-9a-f]{32}$') {
        $machineScripts = Join-Path $env:SystemRoot 'System32\GroupPolicy\Machine\Scripts'
        if (-not $startupScript) {
            $startupScript = Join-Path $machineScripts ('Startup\wukong-provision-' + $provisionId + '.cmd')
        }
        if (-not $scriptsIni) {
            $scriptsIni = Join-Path $machineScripts 'scripts.ini'
        }
        if (-not $scriptsIniBackup) {
            $scriptsIniBackup = Join-Path $machineScripts ('scripts.ini.wukong-lab-' + $provisionId + '.bak')
        }
        if (-not $gptIni) {
            $gptIni = Join-Path $env:SystemRoot 'System32\GroupPolicy\gpt.ini'
        }
        if (-not $gptIniBackup) {
            $gptIniBackup = Join-Path $env:SystemRoot ('System32\GroupPolicy\gpt.ini.wukong-lab-' + $provisionId + '.bak')
        }
        $startupEntry = 'wukong-provision-' + $provisionId + '.cmd'
    }
    # Remove the machine startup injection before anything else so a later boot
    # can never repeat the installation.
    try {
        $restored = $false
        if ($scriptsIni) {
            if (Test-Path -LiteralPath $scriptsIniBackup) {
                Restore-PolicyFile $scriptsIni $scriptsIniBackup | Out-Null
                $restored = $true
            } elseif ($startupEntry) {
                Remove-StartupEntry $scriptsIni $startupEntry | Out-Null
            }
        }
        if ($gptIni -and (Test-Path -LiteralPath $gptIniBackup)) {
            # Without a backup the wrapper leaves gpt.ini alone: it never deletes
            # policy content it cannot restore, and an injected version value is
            # harmless once the startup entry is gone.
            Restore-PolicyFile $gptIni $gptIniBackup | Out-Null
            $restored = $true
        }
        if ($startupScript -and (Test-Path -LiteralPath $startupScript)) {
            Remove-Item -LiteralPath $startupScript -Force
        }
        $receipt.startup_script_deleted = [bool]$startupScript
        $receipt.startup_cleanup = if ($restored) { 'restored' } else { 'removed' }
    } catch {
        $receipt.startup_cleanup = 'failed:' + [string]$_.Exception.Message
    }
    try {
        # The wrapper cannot delete the running script itself; a detached helper
        # removes the script and plan after this process exits. Guest shutdown
        # waits long enough for the deletion to finish.
        $arguments = '/d /c choice /t 3 /d y >nul & del /f /q ' + $planPath + ' ' + $PSCommandPath
        Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\cmd.exe') -ArgumentList $arguments -WindowStyle Hidden | Out-Null
        $receipt.cleanup_scheduled = $true
    } catch {
        $receipt.cleanup_scheduled = $false
    }
    if (-not $existingReceipt) {
        Save-Receipt
    }
    try {
        & shutdown.exe /s /t 10 /f | Out-Null
    } catch {
    }
}
