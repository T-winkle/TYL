param(
    [Parameter(Mandatory)][int]$AppProcessId,
    [Parameter(Mandatory)][string]$Scenario,
    [Parameter(Mandatory)][string]$OutputPath,
    [int]$DurationSeconds = 0,
    [int]$IntervalSeconds = 5,
    [string]$StageLogPath,
    [switch]$UntilExit
)

# Private bytes measure committed private memory; working sets include shared pages.
# Capture descendants, not every WebView2 on the desktop. No working-set trimming.
$ErrorActionPreference = 'Stop'
$watch = [Diagnostics.Stopwatch]::StartNew()
do {
    $allProcesses = @(Get-CimInstance Win32_Process)
    $root = $allProcesses | Where-Object ProcessId -eq $AppProcessId
    if (-not $root -or $root.Name -ne 'tyl-app.exe') {
        if ($UntilExit) { break }
        throw 'TYL process no longer exists'
    }
    $sampleScenario = $Scenario
    if ($StageLogPath -and (Test-Path -LiteralPath $StageLogPath)) {
        $stagePattern = '\[memory-bench\] stage=([^ ]+) mode=' + [regex]::Escape($Scenario) + '$'
        $lastStage = Get-Content -LiteralPath $StageLogPath -Tail 150 | Select-String $stagePattern | Select-Object -Last 1
        if ($lastStage) { $sampleScenario = "$Scenario/$($lastStage.Matches[0].Groups[1].Value)" }
    }
    $ids = [Collections.Generic.HashSet[int]]::new()
    [void]$ids.Add($AppProcessId)
    do {
        $added = $false
        foreach ($entry in $allProcesses) {
            if ($ids.Contains([int]$entry.ParentProcessId) -and $ids.Add([int]$entry.ProcessId)) {
                $added = $true
            }
        }
    } while ($added)
    $rows = foreach ($entry in $allProcesses) {
        if (-not $ids.Contains([int]$entry.ProcessId)) { continue }
        $proc = Get-Process -Id $entry.ProcessId -ErrorAction SilentlyContinue
        if (-not $proc) { continue }
        $role = $entry.Name
        if ($entry.Name -eq 'msedgewebview2.exe') {
            $role = 'browser'
            if ($entry.CommandLine -match '--type=([^\s"]+)') { $role = $Matches[1] }
        }
        [pscustomobject]@{
            TimestampUtc = [DateTime]::UtcNow.ToString('o')
            Scenario = $sampleScenario
            ElapsedSeconds = [math]::Round($watch.Elapsed.TotalSeconds, 1)
            RootPid = $AppProcessId
            Pid = $entry.ProcessId
            Role = $role
            PrivateMiB = [math]::Round($proc.PrivateMemorySize64 / 1MB, 2)
            WorkingSetMiB = [math]::Round($proc.WorkingSet64 / 1MB, 2)
            CpuSeconds = [math]::Round($proc.TotalProcessorTime.TotalSeconds, 3)
        }
    }
    $rows | Export-Csv -LiteralPath $OutputPath -NoTypeInformation -Append -Encoding utf8
    $rows | Format-Table Scenario,ElapsedSeconds,Pid,Role,PrivateMiB,WorkingSetMiB -AutoSize
    if ($UntilExit -and $DurationSeconds -gt 0 -and $watch.Elapsed.TotalSeconds -ge $DurationSeconds) {
        throw "Benchmark exceeded the $DurationSeconds second limit"
    }
    if (-not $UntilExit -and $watch.Elapsed.TotalSeconds -ge $DurationSeconds) { break }
    $remaining = if ($UntilExit) { $IntervalSeconds } else { $DurationSeconds - $watch.Elapsed.TotalSeconds }
    Start-Sleep -Milliseconds ([int](1000 * [math]::Min($IntervalSeconds, $remaining)))
} while ($true)
