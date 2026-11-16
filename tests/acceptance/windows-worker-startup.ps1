# Pure diagnostic validation, also exercised without installing a worker.
function Read-ReadyWorkerStartup([string]$Root,[string]$Installation,[string]$Package){
    $metadata=Join-Path $Root 'metadata'
    $reportPath=Join-Path $metadata 'startup-diagnostics.json'
    $summaryPath=Join-Path $metadata 'last-service-error.txt'
    foreach($entry in @(@($reportPath,16384),@($summaryPath,1024))){
        $file=Get-Item -LiteralPath $entry[0] -ErrorAction Stop
        if($file.Length -gt $entry[1] -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)){
            throw 'Startup diagnostic exceeds its bound or is a reparse point'
        }
    }
    $value=Get-Content -LiteralPath $reportPath -Raw -Encoding utf8 | ConvertFrom-Json
    if($value.schema -ne 2 -or $value.installation_id -cne $Installation -or $value.package_sha256 -cne $Package -or $value.attempt_id -cnotmatch '^[0-9a-f]{32}$' -or $value.ready -cne $true -or $value.execution_boundary -cne 'disposable_vm' -or $value.external_isolation_verified -cne $false){
        throw 'Startup diagnostic identity or readiness differs'
    }
    $stages=@('deployment_identity','resource_recovery','dependency_selftest','guest_controls_selftest','communication_ready')
    $events=@($value.events)
    if($events.Count -ne 10){throw 'Startup stages are incomplete'}
    for($index=0;$index -lt 10;$index++){
        $status=if($index % 2 -eq 0){'started'}else{'succeeded'}
        if($events[$index].stage -cne $stages[[int][Math]::Floor($index/2)] -or $events[$index].status -cne $status){throw 'Startup stage did not succeed'}
    }
    $summary=Get-Content -LiteralPath $summaryPath -Raw -Encoding utf8
    $expected='\Aattempt='+[regex]::Escape($value.attempt_id)+' installation='+[regex]::Escape($Installation)+' code=startup_ready elapsed_ms=[0-9]+\r?\n\z'
    if($summary -cnotmatch $expected){throw 'Startup summary is stale or records a lifecycle failure'}
    return $value
}
