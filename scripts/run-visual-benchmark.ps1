param(
    [ValidateSet('visual-on', 'visual-off')][string]$Mode = 'visual-off',
    [string]$Executable = 'D:\Code\tyl\target\release\tyl-app.exe',
    [string]$OutputDirectory = 'D:\Code\tyl\target\visual-bench'
)
# Requires a Release binary built with --features memory-bench.
# Uses generated fixtures only; never reads or modifies the user's settings.
$ErrorActionPreference = 'Stop'
if (Get-Process tyl-app -ErrorAction SilentlyContinue) { throw 'Exit TYL before running this test.' }
if ($env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -match '--disable-gpu(\s|$)') {
    throw 'Remove the inherited GPU override before comparing modes.'
}
$runDirectory = Join-Path $OutputDirectory ((Get-Date -Format 'yyyyMMdd-HHmmss') + "-$Mode")
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
$exePath = Join-Path $runDirectory 'tyl-app.exe'
Copy-Item -LiteralPath (Resolve-Path -LiteralPath $Executable).Path -Destination $exePath
@{ gpu_acceleration = ($Mode -eq 'visual-on'); theme = 'light'; color_scheme = 'indigo' } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $runDirectory 'settings.json') -Encoding utf8NoBOM
$previousMode = $env:TYL_MEMORY_BENCH
try {
    $env:TYL_MEMORY_BENCH = $Mode
    $proc = Start-Process -FilePath $exePath -WindowStyle Hidden -PassThru
    Write-Output "Renderer QA mode=$Mode pid=$($proc.Id) directory=$runDirectory"
    Start-Sleep -Seconds 3
    $processes = @(Get-CimInstance Win32_Process)
    $ids = @($proc.Id)
    do {
        $children = @($processes | Where-Object { $_.ParentProcessId -in $ids -and $_.ProcessId -notin $ids })
        $ids += @($children | ForEach-Object ProcessId)
    } while ($children.Count -gt 0)
    $processes | Where-Object { $_.ProcessId -in $ids } |
        Select-Object Name, ProcessId, ParentProcessId, CommandLine |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $runDirectory 'processes.json') -Encoding utf8NoBOM
    if (-not $proc.WaitForExit(180000)) { throw "QA timed out; inspect process $($proc.Id). It was not forcibly stopped." }
    $log = @(Get-Content -LiteralPath (Join-Path $runDirectory 'tyl.log'))
    $log | Select-String '\[(gpu|visual-bench|memory-bench)\]' | ForEach-Object Line |
        Set-Content -LiteralPath (Join-Path $runDirectory 'diagnostics.txt') -Encoding utf8NoBOM
    if (($log | Select-String '\[memory-bench\] ERROR') -or
        -not ($log | Select-String "stage=complete mode=$Mode")) { throw 'Renderer QA failed. Inspect diagnostics.' }
    $samples = @($log | Select-String '\[visual-bench\] \{' | ForEach-Object {
        ($_.Line -replace '^.*?\[visual-bench\] ', '') | ConvertFrom-Json
    })
    if ($samples.Count -ne 8 -or @($samples | Where-Object { $_.visibilityChanged -or $_.visibility -ne 'visible' }).Count) {
        throw 'Missing or backgrounded motion samples; do not use this run for comparison.'
    }
    $samples | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $runDirectory 'motion.json') -Encoding utf8NoBOM
    $samples | Format-Table scene, phase, frames, median_ms, p95_ms, max_ms, over_33ms, scrollExtent
    Write-Output "Renderer QA complete: $runDirectory"
} finally {
    $env:TYL_MEMORY_BENCH = $previousMode
}
