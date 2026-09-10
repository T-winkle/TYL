param(
    [ValidateSet('baseline', 'low', 'visibility', 'opaque', 'gpu-on', 'gpu-off')][string[]]$Modes = @('baseline', 'low', 'opaque'),
    [string]$Executable = 'D:\Code\tyl\target\memory-bench\tyl-app.exe',
    [string]$OutputDirectory = 'D:\Code\tyl\target\memory-bench'
)
$ErrorActionPreference = 'Stop'
if (Get-Process tyl-app -ErrorAction SilentlyContinue) { throw 'Exit the existing TYL instance before benchmarking.' }
$exePath = (Resolve-Path -LiteralPath $Executable).Path
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$logPath = Join-Path (Split-Path -Parent $exePath) 'tyl.log'
$previousMode = $env:TYL_MEMORY_BENCH
$previousBrowserArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
if (($Modes -contains 'gpu-on' -or $Modes -contains 'gpu-off') -and $previousBrowserArguments -match '--disable-gpu(\s|$)') {
    throw 'The parent environment already disables GPU; remove that override before the A/B test.'
}
try {
    foreach ($mode in $Modes) {
        $runId = Get-Date -Format 'yyyyMMdd-HHmmss'
        $csv = Join-Path $OutputDirectory "$runId-$mode.csv"
        $oldLines = if (Test-Path -LiteralPath $logPath) { @(Get-Content -LiteralPath $logPath).Count } else { 0 }
        $env:TYL_MEMORY_BENCH = $mode
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $previousBrowserArguments
        if ($mode -eq 'gpu-off') {
            $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "$previousBrowserArguments --disable-gpu".Trim()
        }
        $proc = Start-Process -FilePath $exePath -WindowStyle Hidden -PassThru
        Write-Output "Benchmark mode=$mode pid=$($proc.Id) csv=$csv"
        & "$PSScriptRoot/measure-memory.ps1" -AppProcessId $proc.Id -Scenario $mode -OutputPath $csv -StageLogPath $logPath -UntilExit -IntervalSeconds 5 -DurationSeconds 900
        # Export only diagnostic tags, not selected text, settings, credentials, or URLs.
        $runLog = @(Get-Content -LiteralPath $logPath | Select-Object -Skip $oldLines)
        $runLog |
            Select-String '\[(memory-bench|webview-memory)\]' |
            ForEach-Object { $_.Line } |
            Set-Content -LiteralPath (Join-Path $OutputDirectory "$runId-$mode.txt") -Encoding utf8
        if (($runLog | Select-String '\[memory-bench\] ERROR') -or
            -not ($runLog | Select-String "stage=complete mode=$mode")) {
            throw "Benchmark $mode did not complete successfully; inspect its diagnostic log."
        }
        Start-Sleep -Seconds 5
    }
} finally {
    $env:TYL_MEMORY_BENCH = $previousMode
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $previousBrowserArguments
}
