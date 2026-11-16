# Registers a per-user scheduled task that runs curate_run.ps1 every 4 hours.
# Remove with:  Unregister-ScheduledTask -TaskName KarukanCurate -Confirm:$false
$run = Join-Path $PSScriptRoot 'curate_run.ps1'
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$run`""
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).Date.AddHours(1) `
    -RepetitionInterval (New-TimeSpan -Hours 4)
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -DontStopOnIdleEnd `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 40) -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'KarukanCurate' -Action $action -Trigger $trigger `
    -Settings $settings -Description 'Karukan IME: turn corrections into dictionary entries (idle only)' -Force | Out-Null
Get-ScheduledTask -TaskName 'KarukanCurate' | Select-Object TaskName, State
