$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "build-windows-package.ps1 requires Windows"
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot
cargo build --release -p race-overlay

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    Join-Path $RepoRoot "target"
}
if (-not [System.IO.Path]::IsPathRooted($TargetDir)) {
    $TargetDir = Join-Path $RepoRoot $TargetDir
}

$Architecture = $env:PROCESSOR_ARCHITECTURE.ToLowerInvariant()
$PackageName = "race-overlay-windows-$Architecture"
$PackageDir = Join-Path $TargetDir "release/$PackageName"
$LicenseDir = Join-Path $PackageDir "licenses"
if (Test-Path $PackageDir) {
    Remove-Item $PackageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $LicenseDir | Out-Null
Copy-Item (Join-Path $TargetDir "release/race-overlay.exe") $PackageDir -Force
Copy-Item "LICENSE" (Join-Path $LicenseDir "RACE_OVERLAY_LICENSE.txt") -Force
Copy-Item "THIRD_PARTY_NOTICES.md" $PackageDir -Force

if ($env:RACE_OVERLAY_FFMPEG_BUNDLE) {
    $BundleDir = $env:RACE_OVERLAY_FFMPEG_BUNDLE
    $FfmpegDir = Join-Path $PackageDir "third-party/ffmpeg"
    foreach ($RelativePath in @(
        "bin/ffmpeg.exe",
        "bin/ffprobe.exe",
        "LICENSE.txt",
        "BUILD_INFO.txt",
        "SOURCE.txt"
    )) {
        if (-not (Test-Path (Join-Path $BundleDir $RelativePath) -PathType Leaf)) {
            throw "FFmpeg bundle is missing $RelativePath"
        }
    }
    $SourceDir = Join-Path $BundleDir "source"
    if (-not (Test-Path $SourceDir -PathType Container) -or
        -not (Get-ChildItem $SourceDir -File -Recurse | Select-Object -First 1)) {
        throw "FFmpeg bundle must include its corresponding source in source/"
    }
    $BuildInfo = Get-Content (Join-Path $BundleDir "BUILD_INFO.txt") -Raw
    if (-not $BuildInfo.Contains("--disable-gpl") -or
        -not $BuildInfo.Contains("--disable-nonfree") -or
        $BuildInfo -match "--enable-(gpl|nonfree)") {
        throw "FFmpeg bundle must explicitly disable GPL and nonfree components"
    }
    New-Item -ItemType Directory -Force -Path $FfmpegDir | Out-Null
    Copy-Item (Join-Path $BundleDir "*") $FfmpegDir -Recurse -Force
    Write-Host "Bundled FFmpeg from $BundleDir"
} else {
    Write-Host "FFmpeg was not bundled; the app will use an installed copy"
}

$Archive = Join-Path $TargetDir "release/$PackageName.zip"
Compress-Archive -Path (Join-Path $PackageDir "*") -DestinationPath $Archive -Force
Write-Host "Built $Archive"
