param(
    [Parameter(Mandatory)][ValidateSet('Block','Restart','Cleanup')][string]$Mode,
    [Parameter(Mandatory)][string]$Plan,
    [switch]$ConfirmHostedRunner
)
$ErrorActionPreference='Stop'
if(-not $ConfirmHostedRunner -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Global worker fixture actions require a dedicated GitHub-hosted Windows runner'
}
$principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if(-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Hosted fixture requires elevation'}
$planPath=(Resolve-Path -LiteralPath $Plan).Path
$runnerRoot=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if(-not $planPath.StartsWith($runnerRoot,[StringComparison]::OrdinalIgnoreCase)){throw 'Fixture plan must stay in runner temporary storage'}
if((Get-Item -LiteralPath $planPath).Length -gt 262144){throw 'Fixture plan exceeds limit'}
$fixture=Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json
if($fixture.installation_id -cnotmatch '^[0-9a-f]{32}$' -or $fixture.manifest_sha256 -cnotmatch '^[0-9a-f]{64}$'){throw 'Invalid fixture identity'}
$ownedParent=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'WukongWorkers'
$ownedRoot=[IO.Path]::GetFullPath((Join-Path $ownedParent $fixture.installation_id))
if($ownedRoot -ne [IO.Path]::GetFullPath($fixture.installed_root)){throw 'Fixture installation root differs from its fixed owned path'}
$serviceName='WukongWorker-'+$fixture.installation_id
$rulePrefix='WukongHostedAcceptance-'+$fixture.installation_id+'-'
$description='Wukong dedicated hosted fixture '+$fixture.installation_id
$marker=Join-Path (Split-Path -Parent $planPath) 'hosted-preflight.json'
$roles=@{Management='wkgm';Worker='wkgw';Debugger='wkgd';Target='wkgt'}
$programs=@($fixture.programs)
$firewall=New-Object -ComObject HNetCfg.FwPolicy2
function Get-FixtureRule([string]$Name){
    try{return $firewall.Rules.Item($Name)}catch{
        $failure=$_.Exception
        while($null -ne $failure){
            if($failure.HResult -eq -2147024894){return $null} # HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)
            $failure=$failure.InnerException
        }
        throw
    }
}
if($programs.Count -lt 1 -or $programs.Count -gt 256){throw 'Invalid fixture executable inventory'}
$mediaRoot=[IO.Path]::GetFullPath($fixture.media).TrimEnd('\')+'\'
foreach($program in $programs){
    $full=[IO.Path]::GetFullPath($program)
    if(-not [IO.Path]::IsPathFullyQualified($program) -or (-not $full.StartsWith($mediaRoot,[StringComparison]::OrdinalIgnoreCase) -and -not $full.StartsWith($ownedRoot+'\',[StringComparison]::OrdinalIgnoreCase))){throw 'Fixture firewall program escapes owned media/installation'}
}
if($Mode -eq 'Block'){
    if(Test-Path -LiteralPath $marker){throw 'Fixture network setup cannot be repeated'}
    if(Test-Path -LiteralPath $ownedRoot){throw 'Fixture cannot adopt an existing installation'}
    if(Get-Service -Name $serviceName -ErrorAction SilentlyContinue){throw 'Fixture service already exists'}
    foreach($role in $roles.Keys){
        if(Get-LocalUser -Name ($roles[$role]+$fixture.installation_id.Substring(0,16)) -ErrorAction SilentlyContinue){throw 'Fixture account already exists'}
    }
    for($index=0;$index -lt $programs.Count;$index++){
        if(Get-FixtureRule ($rulePrefix+$index)){throw 'Fixture rule already exists'}
    }
    if(Get-FixtureRule $serviceName){throw 'Installation SSH rule already exists'}
    $proofFile=[IO.File]::Open($marker,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try{
        $proofBytes=[Text.Encoding]::UTF8.GetBytes('{"installation_id":"'+$fixture.installation_id+'"}')
        $proofFile.Write($proofBytes,0,$proofBytes.Length)
        $proofFile.Flush($true)
    }finally{$proofFile.Dispose()}
    # Keep the CI agent online. Deny external addresses for every shipped
    # executable at both media and installed paths. Exclude loopback and this
    # machine's explicitly enrolled management address, never its whole subnet.
    $external=@($fixture.external_addresses)
    if($external.Count -lt 3 -or $external.Count -gt 5){throw 'Invalid fixture external address inventory'}
    foreach($range in $external){if($range -cnotmatch '^[0-9a-fA-F:.]+-[0-9a-fA-F:.]+$'){throw 'Invalid fixture address range'}}
    for($index=0;$index -lt $programs.Count;$index++){
        $workerRule=New-Object -ComObject HNetCfg.FWRule
        $workerRule.Name=$rulePrefix+$index
        $workerRule.Description=$description
        $workerRule.Grouping='Wukong Hosted Acceptance'
        $workerRule.Direction=2
        $workerRule.Action=0
        $workerRule.Enabled=$true
        $workerRule.Profiles=2147483647
        $workerRule.ApplicationName=$programs[$index]
        $workerRule.RemoteAddresses=$external -join ','
        $firewall.Rules.Add($workerRule)
    }
    @{blocked_program_paths=$programs.Count;management_address=$fixture.management_ip;scope='external_worker_executable_network_not_whole_vm'} | ConvertTo-Json -Compress
    exit 0
}
if(-not (Test-Path -LiteralPath $marker)){throw 'No cold preflight proof; refusing resource cleanup or restart'}
$proof=Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
if($proof.installation_id -ne $fixture.installation_id){throw 'Fixture ownership proof changed'}
$service=Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if($service){
    $image=(Get-ItemProperty -LiteralPath ('HKLM:\SYSTEM\CurrentControlSet\Services\'+$serviceName) -Name ImagePath).ImagePath
    $expected=Join-Path $ownedRoot ('versions\'+$fixture.manifest_sha256+'\runtime\native\wukong-worker.exe')
    if(-not $image.StartsWith('"'+$expected+'" ',[StringComparison]::OrdinalIgnoreCase)){throw 'Service executable differs from owned fixture deployment'}
    if($service.Status -ne [ServiceProcess.ServiceControllerStatus]::Stopped){
        $service.Stop()
        $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Stopped,[TimeSpan]::FromSeconds(45))
    }
}
if($Mode -eq 'Restart'){
    if(-not $service){throw 'Installed fixture service disappeared'}
    if(Test-Path -LiteralPath (Join-Path $ownedRoot 'metadata/last-service-error.txt')){throw 'Native service recorded a lifecycle failure'}
    $service.Start()
    $service.WaitForStatus([ServiceProcess.ServiceControllerStatus]::Running,[TimeSpan]::FromSeconds(210))
    @{restarted=$true;scope='owned_native_service'} | ConvertTo-Json -Compress
    exit 0
}
if($service){
    & (Join-Path $env:SystemRoot 'System32/sc.exe') delete $serviceName | Out-Null
    if($LASTEXITCODE){throw 'Cannot delete owned stopped fixture service'}
    $service.Dispose()
}
foreach($role in $roles.Keys){
    $name=$roles[$role]+$fixture.installation_id.Substring(0,16)
    $account=Get-LocalUser -Name $name -ErrorAction SilentlyContinue
    if($account){
        if($account.Description -cne ('Wukong owned '+$fixture.installation_id+' '+$role)){throw 'Fixture account ownership comment differs'}
        Remove-LocalUser -SID $account.SID
    }
}
for($index=0;$index -lt $programs.Count;$index++){
    $rule=Get-FixtureRule ($rulePrefix+$index)
    if($rule){
        if($rule.Description -cne $description -or $rule.ApplicationName -ine $programs[$index] -or $rule.Direction -ne 2 -or $rule.Action -ne 0){throw 'Fixture network rule ownership differs'}
        $firewall.Rules.Remove($rulePrefix+$index)
    }
}
$sshRule=Get-FixtureRule $serviceName
if($sshRule){
    $expectedSsh=Join-Path $ownedRoot ('versions\'+$fixture.manifest_sha256+'\runtime\openssh\sshd.exe')
    if($sshRule.Description -cne 'Wukong owned offline worker SSH management' -or $sshRule.ApplicationName -ine $expectedSsh){throw 'Owned SSH rule differs'}
    $firewall.Rules.Remove($serviceName)
}
if(Test-Path -LiteralPath $ownedRoot){
    $intent=Join-Path $ownedRoot 'metadata/installation-intent.json'
    if(Test-Path -LiteralPath $intent){
        $ownership=Get-Content -LiteralPath $intent -Raw | ConvertFrom-Json
        if($ownership.installation_id -ne $fixture.installation_id -or $ownership.manifest_sha256 -ne $fixture.manifest_sha256){throw 'Fixture protected installation intent differs'}
        $resolved=(Resolve-Path -LiteralPath $ownedRoot).Path
        if($resolved -ne $ownedRoot -or (Split-Path -Parent $resolved) -ne [IO.Path]::GetFullPath($ownedParent)){throw 'Refusing deletion outside the exact owned installation'}
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
@{cleaned_owned_os_resources=$true;remaining_task_root=(Test-Path -LiteralPath $ownedRoot);ephemeral_lsa_secrets_retire_with_hosted_vm=$true;production_uninstall_verified=$false} | ConvertTo-Json -Compress
