[CmdletBinding()]
param(
    [ValidateSet('info', 'python', 'node')][string]$Action = 'info',
    [string]$Script,
    [string[]]$ScriptArguments = @()
)
$ErrorActionPreference = 'Stop'

# This launcher is a normal run_command child, not an authorization or sandbox layer.
# The host still owns approval, timeout, cancellation, and process-tree cleanup.
$artifactWorkspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
function Resolve-ArtifactScript([string]$RelativePath) {
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [IO.Path]::IsPathRooted($RelativePath)) {
        throw 'Artifact script must be workspace-relative.'
    }
    $artifactParts = $RelativePath -split '[/\\]'
    if ($artifactParts -contains '..' -or $RelativePath.Contains(':')) {
        throw 'Artifact script traversal and alternate streams are forbidden.'
    }
    $artifactTarget = [IO.Path]::GetFullPath((Join-Path $artifactWorkspace $RelativePath))
    if (-not $artifactTarget.StartsWith($artifactWorkspace + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Artifact script is outside the workspace.'
    }
    $artifactCursor = Get-Item -LiteralPath $artifactTarget -Force
    if ($artifactCursor.PSIsContainer) { throw 'Artifact script must be a file.' }
    while ($null -ne $artifactCursor) {
        if ($artifactCursor.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'Artifact script links and directory junctions are forbidden.'
        }
        $artifactCursor = $(if ($artifactCursor -is [IO.FileInfo]) { $artifactCursor.Directory } else { $artifactCursor.Parent })
        if ($null -eq $artifactCursor) { break }
    }
    return $artifactTarget
}

# Only the known installed dependency layout is probed; no installation, broad
# filesystem search, credentials, external writes, or workspace junctions.
$artifactDependencies = Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cache/codex-runtimes/codex-primary-runtime/dependencies'
$artifactNode = Join-Path $artifactDependencies 'node/bin/node.exe'
$artifactPython = Join-Path $artifactDependencies 'python/python.exe'
$artifactModules = Join-Path $artifactDependencies 'node/node_modules'
$artifactPackage = Join-Path $artifactModules '@oai/artifact-tool/package.json'
$artifactResult = [ordered]@{
    schemaVersion = 1
    workspace = $artifactWorkspace
    node = $(if (Test-Path -LiteralPath $artifactNode -PathType Leaf) { $artifactNode } else { $null })
    python = $(if (Test-Path -LiteralPath $artifactPython -PathType Leaf) { $artifactPython } else { $null })
    artifactToolVersion = $(if (Test-Path -LiteralPath $artifactPackage -PathType Leaf) { (Get-Content -LiteralPath $artifactPackage -Raw | ConvertFrom-Json).version } else { $null })
    libreOffice = $(if (Get-Command soffice -ErrorAction SilentlyContinue) { (Get-Command soffice).Source } else { $null })
}
if ($Action -eq 'info') { $artifactResult | ConvertTo-Json; exit 0 }
$artifactScript = Resolve-ArtifactScript $Script
Set-Location -LiteralPath $artifactWorkspace
if ($Action -eq 'python') {
    if (-not $artifactResult.python) { throw 'Bundled Python is unavailable; prepare an installed artifact runtime first.' }
    if ([IO.Path]::GetExtension($artifactScript) -ne '.py') { throw 'Python requires a .py script.' }
    & $artifactPython $artifactScript @ScriptArguments
} else {
    if (-not $artifactResult.node -or -not $artifactResult.artifactToolVersion) { throw 'Bundled Node/artifact-tool is unavailable; prepare an installed artifact runtime first.' }
    if ([IO.Path]::GetExtension($artifactScript) -notin @('.mjs', '.js', '.cjs')) { throw 'Node requires a JavaScript script.' }
    $artifactLoader = Resolve-ArtifactScript 'scripts/plugin-artifact-register.mjs'
    $artifactPreviousModules = $env:K_CODER_ARTIFACT_NODE_MODULES
    try {
        $env:K_CODER_ARTIFACT_NODE_MODULES = $artifactModules
        & $artifactNode --import ([Uri]$artifactLoader).AbsoluteUri $artifactScript @ScriptArguments
    } finally {
        $env:K_CODER_ARTIFACT_NODE_MODULES = $artifactPreviousModules
    }
}
exit $LASTEXITCODE
