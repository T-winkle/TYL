param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$OutputPath,
    [int]$DurationSeconds = 1800
)
$ErrorActionPreference = 'Stop'
$watch = [Diagnostics.Stopwatch]::StartNew()
while ($watch.Elapsed.TotalSeconds -lt $DurationSeconds) {
    $allProcesses = @(Get-CimInstance Win32_Process)
    $root = $allProcesses | Where-Object ExecutablePath -eq $Executable | Select-Object -First 1
    if ($root) {
        $ids = [Collections.Generic.HashSet[int]]::new()
        [void]$ids.Add([int]$root.ProcessId)
        do {
            $added = $false
            foreach ($entry in $allProcesses) {
                if ($ids.Contains([int]$entry.ParentProcessId) -and $ids.Add([int]$entry.ProcessId)) { $added = $true }
            }
        } while ($added)
        $stage = ''
        $logPath = Join-Path (Split-Path -Parent $Executable) 'tyl.log'
        $lastStage = Get-Content -LiteralPath $logPath -Tail 150 | Select-String '\[memory-bench\] stage=([^ ]+) mode=([^ ]+)' | Select-Object -Last 1
        if ($lastStage) { $stage = "$($lastStage.Matches[0].Groups[2].Value)/$($lastStage.Matches[0].Groups[1].Value)" }
        # A successful enumeration with no matching adapters is a measured absence,
        # not a missing sample. Enumeration failures still terminate the collector.
        $rows = @(foreach ($counter in Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory) {
            if ($counter.Name -notmatch '^pid_(\d+)_') { continue }
            $gpuPid = [int]$Matches[1]
            if (-not $ids.Contains($gpuPid)) { continue }
            [pscustomobject]@{
                TimestampUtc = [DateTime]::UtcNow.ToString('o')
                Scenario = $stage
                RootPid = $root.ProcessId
                Pid = $gpuPid
                Adapter = $counter.Name
                DedicatedMiB = [math]::Round($counter.DedicatedUsage / 1MB, 2)
                SharedMiB = [math]::Round($counter.SharedUsage / 1MB, 2)
                CommittedMiB = [math]::Round($counter.TotalCommitted / 1MB, 2)
            }
        })
        if (-not $rows.Count) {
            $rows = @([pscustomobject]@{
                TimestampUtc = [DateTime]::UtcNow.ToString('o')
                Scenario = $stage
                RootPid = $root.ProcessId
                Pid = $root.ProcessId
                Adapter = 'no-app-adapter-counters'
                DedicatedMiB = 0
                SharedMiB = 0
                CommittedMiB = 0
            })
        }
        if ($rows) { $rows | Export-Csv -LiteralPath $OutputPath -Append -NoTypeInformation -Encoding utf8 }
    }
    Start-Sleep -Seconds 5
}
