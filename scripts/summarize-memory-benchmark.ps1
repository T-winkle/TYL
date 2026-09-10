param([Parameter(Mandatory)][string]$Directory)
$ErrorActionPreference = 'Stop'
function Get-Sum($rows, $property) {
    [math]::Round(($rows | Measure-Object -Property $property -Sum).Sum, 2)
}
foreach ($file in Get-ChildItem -LiteralPath $Directory -Filter '*.csv') {
    if ($file.Name -eq 'gpu.csv') { continue }
    $rows = @(Import-Csv -LiteralPath $file.FullName)
    if (-not $rows.Count) { continue }
    $buckets = [Collections.Generic.List[object]]::new()
    $bucket = [Collections.Generic.List[object]]::new()
    $previous = [DateTimeOffset]::MinValue
    foreach ($row in $rows) {
        $at = [DateTimeOffset]::Parse($row.TimestampUtc)
        if (($at - $previous).TotalSeconds -gt 2 -and $bucket.Count) {
            $buckets.Add($bucket.ToArray())
            $bucket = [Collections.Generic.List[object]]::new()
        }
        $row.PrivateMiB = [double]$row.PrivateMiB
        $row.WorkingSetMiB = [double]$row.WorkingSetMiB
        $bucket.Add($row)
        $previous = $at
    }
    if ($bucket.Count) { $buckets.Add($bucket.ToArray()) }
    $snapshots = foreach ($sample in $buckets) {
        $webview = @($sample | Where-Object Role -ne 'tyl-app.exe')
        [pscustomobject]@{
            Scenario = $sample[0].Scenario
            TimestampUtc = $sample[0].TimestampUtc
            ElapsedSeconds = $sample[0].ElapsedSeconds
            AppMiB = Get-Sum @($sample | Where-Object Role -eq 'tyl-app.exe') 'PrivateMiB'
            WebViewMiB = Get-Sum $webview 'PrivateMiB'
            GpuMiB = Get-Sum @($sample | Where-Object Role -eq 'gpu-process') 'PrivateMiB'
            RendererMiB = Get-Sum @($sample | Where-Object Role -eq 'renderer') 'PrivateMiB'
            WorkingSetSumMiB = Get-Sum $sample 'WorkingSetMiB'
            RendererCount = @($sample | Where-Object Role -eq 'renderer').Count
        }
    }
    Write-Output $file.Name
    $snapshots | Group-Object Scenario | ForEach-Object {
        $samples = $_.Group
        $samples[0]
        if ($samples.Count -gt 1) { $samples[-1] }
    } | Format-Table Scenario,ElapsedSeconds,AppMiB,WebViewMiB,GpuMiB,RendererMiB,RendererCount -AutoSize

    $logFile = [IO.Path]::ChangeExtension($file.FullName, '.txt')
    if (-not (Test-Path -LiteralPath $logFile)) { continue }
    $presents = @{}
    $durations = [Collections.Generic.List[object]]::new()
    $stage = ''
    foreach ($line in Get-Content -LiteralPath $logFile) {
        if ($line -match 'stage=([^ ]+)') { $stage = $Matches[1] }
        if ($line -match '^\[([^\]]+)\].*present id=(\d+)') {
            $presents[$Matches[2]] = @{ Time = [TimeSpan]::Parse($Matches[1]); Stage = $stage }
        }
        if ($line -match '^\[([^\]]+)\].*frame-ready id=(\d+)') {
            $id = $Matches[2]
            $at = [TimeSpan]::Parse($Matches[1])
            if ($presents.ContainsKey($id)) {
                $ms = ($at - $presents[$id].Time).TotalMilliseconds
                if ($ms -lt 0) { $ms += 86400000 }
                $durations.Add([pscustomobject]@{ Id = $id; Stage = $presents[$id].Stage; ReadyMs = $ms })
            }
        }
    }
    # IPC + double RAF readiness is NOT physical first-paint or translation latency.
    $durations | Format-Table -AutoSize
    foreach ($group in @('all', 'reopen')) {
        $values = @($durations | Where-Object { $group -eq 'all' -or $_.Stage -like 'reopen-*' } | Sort-Object ReadyMs)
        if ($values.Count) {
            [pscustomobject]@{
                Group = $group
                Samples = $values.Count
                MedianMs = $values[[math]::Floor(($values.Count - 1) * 0.5)].ReadyMs
                P95Ms = $values[[math]::Ceiling($values.Count * 0.95) - 1].ReadyMs
                MaxMs = $values[-1].ReadyMs
            } | Format-Table
        }
    }
}
