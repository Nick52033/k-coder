[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ripgrepVersion = "15.2.0"
$archiveUrl = "https://github.com/BurntSushi/ripgrep/releases/download/$ripgrepVersion/ripgrep-$ripgrepVersion-x86_64-pc-windows-msvc.zip"
$expectedArchiveSha256 = "71b2fef860abe467217a538ff31de02f5258807c0129f771846f87bd029aafc5"
$expectedBinarySha256 = "14231169855ec5205cf5a1b6f1db358ff4aed4247c86b69ce8aae647c77f6680"
$destination = [System.IO.Path]::GetFullPath(
    (Join-Path $PSScriptRoot "..\src\resources\tools\windows-x86_64")
)
$temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$stagingRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $temporaryRoot "k-coder-ripgrep-$([guid]::NewGuid())")
)

if (-not $stagingRoot.StartsWith($temporaryRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to use a staging directory outside the system temporary directory."
}

try {
    New-Item -ItemType Directory -Path $stagingRoot | Out-Null
    $archivePath = Join-Path $stagingRoot "ripgrep.zip"
    $extractRoot = Join-Path $stagingRoot "extract"
    Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath

    $archiveSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($archiveSha256 -ne $expectedArchiveSha256) {
        throw "ripgrep archive SHA-256 mismatch: expected $expectedArchiveSha256, got $archiveSha256"
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot
    $sourceRoot = Join-Path $extractRoot "ripgrep-$ripgrepVersion-x86_64-pc-windows-msvc"
    $sourceBinary = Join-Path $sourceRoot "rg.exe"
    $binarySha256 = (Get-FileHash -LiteralPath $sourceBinary -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($binarySha256 -ne $expectedBinarySha256) {
        throw "ripgrep binary SHA-256 mismatch: expected $expectedBinarySha256, got $binarySha256"
    }

    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    foreach ($name in @("rg.exe", "LICENSE-MIT", "UNLICENSE")) {
        Copy-Item -LiteralPath (Join-Path $sourceRoot $name) -Destination (Join-Path $destination $name) -Force
    }
    Write-Output "Installed ripgrep $ripgrepVersion in $destination"
}
finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
    }
}
