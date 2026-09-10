param([Parameter(Mandatory)][string]$Directory)
$ErrorActionPreference = 'Stop'
function Get-Stats($values) {
    $sorted = @($values | ForEach-Object { [double]$_ } | Sort-Object)
    if (-not $sorted.Count) { return $null }
    [pscustomobject]@{
        Count = $sorted.Count
        Min = $sorted[0]
        Median = $sorted[[math]::Floor(($sorted.Count - 1) / 2)]
        Max = $sorted[-1]
    }
}
$gpuRows = @(Import-Csv -LiteralPath (Join-Path $Directory 'gpu.csv'))
foreach ($mode in @('gpu-on', 'gpu-off')) {
    $file = Get-ChildItem -LiteralPath $Directory -Filter "*-$mode.csv" | Select-Object -Last 1
    if (-not $file) { continue }
    $rows = @(Import-Csv -LiteralPath $file.FullName)
    $buckets = [Collections.Generic.List[object]]::new()
    $bucket = [Collections.Generic.List[object]]::new()
    $previous = [DateTimeOffset]::MinValue
    foreach ($row in $rows) {
        $at = [DateTimeOffset]::Parse($row.TimestampUtc)
        if (($at - $previous).TotalSeconds -gt 2 -and $bucket.Count) {
            $buckets.Add($bucket.ToArray())
            $bucket = [Collections.Generic.List[object]]::new()
        }
        $bucket.Add($row)
        $previous = $at
    }
    if ($bucket.Count) { $buckets.Add($bucket.ToArray()) }
    $samples = foreach ($sample in $buckets) {
        [pscustomobject]@{
            Scenario = $sample[0].Scenario
            Elapsed = [double]$sample[0].ElapsedSeconds
            PrivateMiB = [math]::Round(($sample | Measure-Object PrivateMiB -Sum).Sum, 2)
            WorkingSetSumMiB = [math]::Round(($sample | Measure-Object WorkingSetMiB -Sum).Sum, 2)
            GpuPrivateMiB = [math]::Round(($sample | Where-Object Role -eq 'gpu-process' | Measure-Object PrivateMiB -Sum).Sum, 2)
        }
    }
    $late = @($samples | Where-Object Scenario -match '/translation-(1[0-9]|20)$')
    $modeGpu = @($gpuRows | Where-Object Scenario -like "$mode/*")
    $lateGpu = @($modeGpu | Where-Object Scenario -match '/translation-(1[0-9]|20)$')
    # All observed process CPU, including the settings renderer after it exits.
    # Sampling can miss CPU spent just before a process exits; this is not CPU %.
    $cpu = ($rows | Group-Object Pid | ForEach-Object {
        ($_.Group | Measure-Object CpuSeconds -Maximum).Maximum
    } | Measure-Object -Sum).Sum
    [pscustomobject]@{
        Mode = $mode
        LateTranslationPrivateMiB = Get-Stats $late.PrivateMiB
        LateTranslationGpuPrivateMiB = Get-Stats $late.GpuPrivateMiB
        LateTranslationWddmCommittedMiB = Get-Stats $lateGpu.CommittedMiB
        LateTranslationWddmDedicatedMiB = Get-Stats $lateGpu.DedicatedMiB
        LateTranslationWddmSharedMiB = Get-Stats $lateGpu.SharedMiB
        StartupLast = @($samples | Where-Object Scenario -eq "$mode/startup") | Select-Object -Last 1
        HiddenNear60s = @($samples | Where-Object Scenario -eq "$mode/hidden-30s") | Select-Object -Last 1
        WddmHiddenNear60s = @($modeGpu | Where-Object Scenario -eq "$mode/hidden-30s") | Select-Object -Last 1
        ObservedCpuSeconds = [math]::Round($cpu, 3)
        ElapsedSeconds = $samples[-1].Elapsed
        AdapterCounterNames = @($modeGpu.Adapter | Sort-Object -Unique)
    } | ConvertTo-Json -Depth 6
}
