param(
    [Parameter(Mandatory = $true)]
    [string]$SkillName,

    [string]$SourceRoot = $(if ($env:CODEX_SKILLS_DIR) { $env:CODEX_SKILLS_DIR } else { Join-Path $env:USERPROFILE ".codex\skills" }),

    [string]$TargetRoot = $(if ($env:CURSOR_SKILLS_DIR) { $env:CURSOR_SKILLS_DIR } else { Join-Path $env:USERPROFILE ".cursor\skills" }),

    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Resolve-OrCreateDirectory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PathValue
    )

    if (-not (Test-Path -LiteralPath $PathValue)) {
        if ($DryRun) {
            return [System.IO.Path]::GetFullPath($PathValue)
        }
        New-Item -ItemType Directory -Path $PathValue -Force | Out-Null
    }

    return (Resolve-Path -LiteralPath $PathValue).Path
}

function Assert-ChildPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Parent,

        [Parameter(Mandatory = $true)]
        [string]$Child
    )

    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\', '/')
    $childFull = [System.IO.Path]::GetFullPath($Child).TrimEnd('\', '/')
    $prefix = $parentFull + [System.IO.Path]::DirectorySeparatorChar

    if (-not $childFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to operate outside target root. Parent: $parentFull Child: $childFull"
    }
}

if ([string]::IsNullOrWhiteSpace($SkillName)) {
    throw "SkillName is required."
}

$normalizedName = $SkillName.Trim()
$sourceRootResolved = (Resolve-Path -LiteralPath $SourceRoot).Path
$targetRootResolved = Resolve-OrCreateDirectory -PathValue $TargetRoot

$sourcePath = Join-Path $sourceRootResolved $normalizedName
$targetPath = Join-Path $targetRootResolved $normalizedName

if (-not (Test-Path -LiteralPath $sourcePath)) {
    $available = Get-ChildItem -LiteralPath $sourceRootResolved -Directory |
        Select-Object -ExpandProperty Name |
        Sort-Object
    throw "Source skill not found: $sourcePath`nAvailable skills:`n$($available -join "`n")"
}

$sourceResolved = (Resolve-Path -LiteralPath $sourcePath).Path
Assert-ChildPath -Parent $sourceRootResolved -Child $sourceResolved
Assert-ChildPath -Parent $targetRootResolved -Child $targetPath

$action = if (Test-Path -LiteralPath $targetPath) { "updated" } else { "created" }

if ($DryRun) {
    [pscustomobject]@{
        Action = $action
        Source = $sourceResolved
        Target = [System.IO.Path]::GetFullPath($targetPath)
        DryRun = $true
    } | Format-List
    exit 0
}

if (Test-Path -LiteralPath $targetPath) {
    $targetResolved = (Resolve-Path -LiteralPath $targetPath).Path
    Assert-ChildPath -Parent $targetRootResolved -Child $targetResolved
    Remove-Item -LiteralPath $targetResolved -Recurse -Force
}

Copy-Item -LiteralPath $sourceResolved -Destination $targetPath -Recurse -Force

[pscustomobject]@{
    Action = $action
    Source = $sourceResolved
    Target = (Resolve-Path -LiteralPath $targetPath).Path
    DryRun = $false
} | Format-List
