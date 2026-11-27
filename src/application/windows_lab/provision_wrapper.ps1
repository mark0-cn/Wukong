# One-shot offline worker provisioning wrapper injected into a powered-off
# Windows guest and started at boot by the fixed \WukongLabProvision Task
# Scheduler task as NT AUTHORITY\SYSTEM (no login, no local Group Policy
# dependency). This is a fixed controller asset: identities come from
# provision-plan.json beside it and the receipt is written beside the plan.
# The finally block writes the receipt first, unregisters the scheduled task,
# deletes the fixed boot launcher, schedules removal of this script and plan, and
# always powers the guest off.
$ErrorActionPreference = 'Stop'
$taskName = '\WukongLabProvision'
$taskGuid = '{A861DA08-B3C7-44C1-8BAC-579B48F54859}'
$taskFile = Join-Path $env:SystemRoot 'System32\Tasks\WukongLabProvision'
$launcherPath = Join-Path $env:ProgramData 'WukongLab\provision\launch.ps1'
$taskCache = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache'
$treeKey = Join-Path $taskCache ('Tree\WukongLabProvision')
$tasksKey = Join-Path $taskCache ('Tasks\' + $taskGuid)
$bootKey = Join-Path $taskCache ('Boot\' + $taskGuid)
$root = Split-Path -Parent $PSCommandPath
$planPath = Join-Path $root 'provision-plan.json'
$receiptPath = Join-Path $root 'receipt.json'
$existingReceipt = Test-Path -LiteralPath $receiptPath
$receipt = [ordered]@{
    schema = 3
    provision_id = $null
    installed = $false
    already_installed = $false
    exit_code = $null
    connection = $null
    boot_task_removed = $false
    launcher_deleted = $false
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
# True when no fixed scheduled-task artifact (file or TaskCache key) remains.
function Test-BootTaskGone {
    $registered = (Test-Path -LiteralPath $taskFile) -or (Test-Path -LiteralPath $treeKey) -or (Test-Path -LiteralPath $tasksKey) -or (Test-Path -LiteralPath $bootKey)
    return (-not $registered)
}
# Unregister exactly the fixed boot task. `schtasks /Delete` removes the task XML
# file and all three TaskCache keys; deleting this while the task action is
# running is supported and does not terminate the action. Retry briefly because
# the Task Scheduler store can be held during other task operations.
function Remove-BootTask {
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        if (Test-BootTaskGone) {
            return $true
        }
        try {
            & (Join-Path $env:SystemRoot 'System32\schtasks.exe') /Delete /TN $taskName /F 2>&1 | Out-Null
        } catch {
        }
        Start-Sleep -Milliseconds 500
    }
    return (Test-BootTaskGone)
}
function Disable-WukongTargetDefender {
    # This runs only inside a dedicated disposable vulnerability-analysis VM.
    # The fixed baseline is restored after untrusted targets execute.
    $policy = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows Defender'
    $realTime = Join-Path $policy 'Real-Time Protection'
    $spynet = Join-Path $policy 'Spynet'
    New-Item -Path $policy, $realTime, $spynet -Force | Out-Null
    Set-ItemProperty -Path $policy -Name DisableAntiSpyware -Value 1 -Type DWord
    foreach ($name in @('DisableRealtimeMonitoring', 'DisableBehaviorMonitoring', 'DisableOnAccessProtection', 'DisableScanOnRealtimeEnable', 'DisableScriptScanning', 'DisableIOAVProtection')) {
        Set-ItemProperty -Path $realTime -Name $name -Value 1 -Type DWord
    }
    Set-ItemProperty -Path $spynet -Name DisableBlockAtFirstSeen -Value 1 -Type DWord
    Set-ItemProperty -Path $spynet -Name SubmitSamplesConsent -Value 2 -Type DWord
    # Use Defender's supported management API while its service is running;
    # protected service-control and Tamper-Protected registry paths are not used.
    Set-MpPreference -DisableRealtimeMonitoring $true -DisableBehaviorMonitoring $true -DisableScriptScanning $true -DisableIOAVProtection $true -DisableBlockAtFirstSeen $true -SubmitSamplesConsent 2
    $preference = Get-MpPreference
    foreach ($name in @('DisableRealtimeMonitoring', 'DisableBehaviorMonitoring', 'DisableScriptScanning', 'DisableIOAVProtection', 'DisableBlockAtFirstSeen')) {
        if (-not $preference.$name) {
            throw ('failed to disable Windows Defender feature ' + $name)
        }
    }
    if ($preference.SubmitSamplesConsent -ne 2) {
        throw 'failed to disable Windows Defender sample submission'
    }
    foreach ($entry in @(@($policy, 'DisableAntiSpyware'), @($realTime, 'DisableRealtimeMonitoring'), @($realTime, 'DisableBehaviorMonitoring'), @($realTime, 'DisableOnAccessProtection'), @($realTime, 'DisableScanOnRealtimeEnable'), @($realTime, 'DisableScriptScanning'), @($realTime, 'DisableIOAVProtection'), @($spynet, 'DisableBlockAtFirstSeen'))) {
        if ((Get-ItemProperty -Path $entry[0] -Name $entry[1]).$($entry[1]) -ne 1) {
            throw ('failed to disable Windows Defender policy ' + $entry[1])
        }
    }
}
# Wait until the attached WUKONG_OFFLINE volume is visible. A boot-trigger task
# can start before the CD-ROM volume and WMI are ready.
function Find-MediaRoot([string]$label) {
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        try {
            foreach ($disk in @(Get-CimInstance -ClassName Win32_LogicalDisk)) {
                if ([string]$disk.VolumeName -eq $label -and [int]$disk.DriveType -eq 5) {
                    $candidate = [string]$disk.DeviceID + '\'
                    if (Test-Path -LiteralPath (Join-Path $candidate 'WukongWorkerSetup.exe')) {
                        return $candidate
                    }
                }
            }
        } catch {
        }
        Start-Sleep -Seconds 5
    }
    throw 'offline media volume WUKONG_OFFLINE is not attached'
}
$plan = $null
try {
    $plan = Get-Content -LiteralPath $planPath -Raw -ErrorAction Stop | ConvertFrom-Json
    $receipt.provision_id = [string]$plan.provision_id
    # Offline lifecycle writes require a cold, fully closed NTFS volume after
    # every later boot. Fast Startup/hibernation would make guestfish read-only.
    & powercfg.exe /hibernate off | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw ('failed to disable guest hibernation and Fast Startup: exit code ' + $LASTEXITCODE)
    }
    # Baselines are created only after this point, so every restored target VM
    # has the same Defender-disabled vulnerability-analysis policy.
    Disable-WukongTargetDefender
    if ($existingReceipt) {
        $receipt.already_installed = $true
    } else {
        $mediaRoot = Find-MediaRoot ([string]$plan.media_label)
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
        # Windows PowerShell 5.1 turns merged native stderr into ErrorRecords.
        # Keep those records non-terminating just long enough to capture the
        # process output and its real exit code; all wrapper cmdlets remain Stop.
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $verifyText = (& $setup verify-enrollment (Join-Path $mediaRoot 'payload') (Join-Path $mediaRoot 'enrollment') ([string]$plan.controller_public_key) ([string]$plan.controller_version) 2>&1 | Out-String)
            $verifyExitCode = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        if ($verifyExitCode -ne 0) {
            throw ('offline media verification failed with exit code ' + $verifyExitCode + ': ' + $verifyText.Trim())
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
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $installText = (& $setup install-reviewed (Join-Path $mediaRoot 'setup.json') ([string]$plan.controller_public_key) ([string]$plan.enrollment_sha256) 2>&1 | Out-String).Trim()
            $exitCode = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        $receipt.exit_code = $exitCode
        if ($installText.Length -gt 65536) {
            throw 'worker installation output exceeded the controller budget'
        }
        if ($exitCode -ne 0) {
            throw ('worker installation failed with exit code ' + $exitCode + ': ' + $installText)
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
    # Unregister the boot trigger before anything else so a later boot can never
    # repeat the installation. The fixed task name is the only task touched.
    try {
        $receipt.boot_task_removed = [bool](Remove-BootTask)
    } catch {
        $receipt.boot_task_removed = $false
    }
    # Delete the fixed per-operation boot launcher. The task action runs this
    # wrapper, so it can be removed while the wrapper is still executing.
    try {
        if (Test-Path -LiteralPath $launcherPath) {
            Remove-Item -LiteralPath $launcherPath -Force
        }
        $receipt.launcher_deleted = -not (Test-Path -LiteralPath $launcherPath)
    } catch {
        $receipt.launcher_deleted = $false
    }
    try {
        # The wrapper cannot delete the running script itself; a detached helper
        # removes the script and plan after this process exits. Guest shutdown
        # waits long enough for the deletion to finish.
        $arguments = '/d /c choice /t 3 /d y >nul & del /f /q "' + $planPath + '" "' + $PSCommandPath + '"'
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
