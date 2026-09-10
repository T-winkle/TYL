param(
    [switch]$AllowDirty,
    [switch]$AllowUnsigned,
    [switch]$SkipChecks,
    [string]$AdditionalTauriConfig
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$uiDirectory = Join-Path $repoRoot 'apps/tyl-app/ui'
$appDirectory = Join-Path $repoRoot 'apps/tyl-app'
$tauriConfigPath = Join-Path $appDirectory 'src-tauri/tauri.conf.json'
$tauriCommand = Join-Path $uiDirectory 'node_modules/.bin/tauri.cmd'

function Invoke-ReleaseStep {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][scriptblock]$Command
    )
    Write-Host "`n==> $Name" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

if ($env:OS -ne 'Windows_NT') {
    throw 'Windows is required to build and sign the NSIS release.'
}

$runningTyl = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
    if ($_.ProcessName -eq 'tyl-app') { return $true }
    try {
        return $_.Path -and [IO.Path]::GetFileName($_.Path) -match '^TYL_.+_portable\.exe$'
    } catch {
        return $false
    }
})
if ($runningTyl.Count -gt 0) {
    throw 'Exit the running TYL instance from the tray before building.'
}

if (-not $AllowDirty) {
    $changes = @(git -C $repoRoot status --porcelain)
    if ($LASTEXITCODE -ne 0) { throw 'Unable to inspect the Git worktree.' }
    if ($changes.Count -gt 0) {
        throw 'The Git worktree is not clean. Commit the release changes or pass -AllowDirty for a local preview.'
    }
}

Invoke-ReleaseStep 'Install locked frontend and Tauri CLI dependencies' {
    npm --prefix $uiDirectory ci
}

if (-not (Test-Path -LiteralPath $tauriCommand)) {
    throw "Tauri CLI was not installed at $tauriCommand"
}

# tauri::generate_context!() validates frontendDist at Rust compile time, so
# the frontend output must exist before workspace-wide Clippy and tests run.
Invoke-ReleaseStep 'Build frontend assets for Rust checks' {
    npm --prefix $uiDirectory run build
}

if (-not $SkipChecks) {
    Invoke-ReleaseStep 'Check Rust formatting' {
        cargo fmt --manifest-path (Join-Path $repoRoot 'Cargo.toml') --all --check
    }
    Invoke-ReleaseStep 'Run strict Rust linting' {
        cargo clippy --manifest-path (Join-Path $repoRoot 'Cargo.toml') --workspace --all-targets -- -D warnings
    }
    Invoke-ReleaseStep 'Run Rust tests' {
        cargo test --manifest-path (Join-Path $repoRoot 'Cargo.toml') --workspace
    }
    Invoke-ReleaseStep 'Check the frontend' {
        npm --prefix $uiDirectory run check
    }
    Invoke-ReleaseStep 'Audit production frontend dependencies' {
        npm --prefix $uiDirectory audit --omit=dev --audit-level=high
    }
}

Invoke-ReleaseStep 'Build application and NSIS installer' {
    Push-Location $appDirectory
    try {
        $buildArguments = @('build', '--bundles', 'nsis')
        if ($AdditionalTauriConfig) {
            $buildArguments += @('--config', (Resolve-Path -LiteralPath $AdditionalTauriConfig).Path)
        }
        & $tauriCommand @buildArguments
    } finally {
        Pop-Location
    }
}

$version = (Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json).version
$releaseDirectory = Join-Path $repoRoot "target/release-artifacts/$version"
$portableSource = Join-Path $repoRoot 'target/release/tyl-app.exe'
$installerSource = Get-ChildItem -LiteralPath (Join-Path $repoRoot 'target/release/bundle/nsis') -Filter '*-setup.exe' |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not (Test-Path -LiteralPath $portableSource)) {
    throw "Portable executable not found: $portableSource"
}
if (-not $installerSource) {
    throw 'NSIS installer was not generated.'
}

New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
$installerTarget = Join-Path $releaseDirectory "TYL_${version}_x64-setup.exe"
$portableArchive = Join-Path $releaseDirectory "TYL_${version}_x64-portable.zip"
$legacyPortableTarget = Join-Path $releaseDirectory "TYL_${version}_x64-portable.exe"
$portableReadme = Join-Path $repoRoot 'distribution/PORTABLE-README.txt'

if (-not (Test-Path -LiteralPath $portableReadme)) {
    throw "Portable README not found: $portableReadme"
}

Copy-Item -LiteralPath $installerSource.FullName -Destination $installerTarget -Force

$signedExecutables = @($installerTarget, $portableSource)
$invalidSignatures = @($signedExecutables | Where-Object {
    (Get-AuthenticodeSignature -LiteralPath $_).Status -ne 'Valid'
})
if ($invalidSignatures.Count -gt 0 -and -not $AllowUnsigned) {
    throw "Unsigned or invalid artifacts: $($invalidSignatures -join ', '). Configure Authenticode signing or pass -AllowUnsigned for a local preview."
}

$portableStagingRoot = Join-Path ([IO.Path]::GetTempPath()) ("tyl-portable-" + [Guid]::NewGuid().ToString('N'))
$portableFolderName = "TYL_${version}_x64-portable"
$portableFolder = Join-Path $portableStagingRoot $portableFolderName
try {
    New-Item -ItemType Directory -Path $portableFolder -Force | Out-Null
    Copy-Item -LiteralPath $portableSource -Destination (Join-Path $portableFolder 'TYL.exe')
    Copy-Item -LiteralPath $portableReadme -Destination (Join-Path $portableFolder 'README.txt')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $portableFolder 'LICENSE.txt')
    if (Test-Path -LiteralPath $portableArchive) {
        [IO.File]::Delete($portableArchive)
    }
    Compress-Archive -LiteralPath $portableFolder -DestinationPath $portableArchive -CompressionLevel Optimal
} finally {
    if (Test-Path -LiteralPath $portableStagingRoot) {
        [IO.Directory]::Delete($portableStagingRoot, $true)
    }
}

# Remove the legacy loose portable executable created by older versions of this script.
if (Test-Path -LiteralPath $legacyPortableTarget) {
    [IO.File]::Delete($legacyPortableTarget)
}

$artifacts = @($installerTarget, $portableArchive)
$checksumPath = Join-Path $releaseDirectory 'SHA256SUMS.txt'
$checksumLines = $artifacts | ForEach-Object {
    $item = Get-Item -LiteralPath $_
    $hash = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($item.Name)"
}
$checksumLines | Set-Content -LiteralPath $checksumPath -Encoding utf8NoBOM

Write-Host "`nRelease artifacts:" -ForegroundColor Green
Get-ChildItem -LiteralPath $releaseDirectory -File |
    Select-Object Name, Length, LastWriteTime |
    Format-Table -AutoSize
if ($invalidSignatures.Count -gt 0) {
    Write-Warning 'Unsigned release artifacts were generated. Windows may show an Unknown publisher or SmartScreen warning.'
}
