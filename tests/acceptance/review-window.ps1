param([int]$TargetProcess)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Disposable GitHub runner required' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ProcessIdProperty, $TargetProcess)
$deadline = [DateTime]::UtcNow.AddSeconds(25)
$names = @()
while ([DateTime]::UtcNow -lt $deadline) {
    $windows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
    foreach ($window in $windows) {
        $elements = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $names = @($elements | ForEach-Object { $_.Current.Name } | Where-Object { $_ } | Select-Object -First 40)
        $checks = @($elements | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::CheckBox })
        if ($checks.Count -ne 3) { continue }
        foreach ($check in $checks) {
            $toggle = $check.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
            if ($toggle.Current.ToggleState -ne [System.Windows.Automation.ToggleState]::Off) { throw 'Uninstall option was unexpectedly selected' }
        }
        $data = @($checks | Where-Object { $_.Current.Name.StartsWith('Delete local application data and settings') })
        if ($data.Count -ne 1) { throw 'Data cleanup checkbox missing' }
        $data[0].GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle()
        while ([DateTime]::UtcNow -lt $deadline) {
            $buttons = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
            $confirm = @($buttons | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -eq 'Continue uninstall' -and $_.Current.IsEnabled })
            if ($confirm.Count -eq 1) {
                $confirm[0].GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
                Write-Output 'Native UI Automation verified default retention and confirmed data-only scope.'
                exit 0
            }
            Start-Sleep -Milliseconds 100
        }
    }
    Start-Sleep -Milliseconds 200
}
throw ('Native review controls unavailable. Visible text: ' + ($names -join ' | '))
