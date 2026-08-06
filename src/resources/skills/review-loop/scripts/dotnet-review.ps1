param(
    [string]$Root = (Get-Location).Path,
    [string]$Solution,
    [string]$Project,
    [string]$TestProject,
    [switch]$RunTests,
    [switch]$BuildSolution
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-FirstMatch {
    param(
        [string]$BasePath,
        [string]$Filter
    )

    $items = Get-ChildItem -LiteralPath $BasePath -Filter $Filter -Recurse -File -ErrorAction SilentlyContinue |
        Sort-Object FullName

    if ($items.Count -gt 0) {
        return $items[0].FullName
    }

    return $null
}

function Resolve-BuildTarget {
    param(
        [string]$BasePath,
        [string]$SolutionPath,
        [string]$ProjectPath,
        [bool]$PreferSolution
    )

    if ($ProjectPath) {
        return @{
            Type = "project"
            Path = (Resolve-Path -LiteralPath $ProjectPath).Path
        }
    }

    if ($SolutionPath) {
        return @{
            Type = "solution"
            Path = (Resolve-Path -LiteralPath $SolutionPath).Path
        }
    }

    $rootLevelSolution = Get-ChildItem -LiteralPath $BasePath -Filter "*.sln" -File -ErrorAction SilentlyContinue |
        Sort-Object FullName |
        Select-Object -First 1

    if ($rootLevelSolution) {
        return @{
            Type = "solution"
            Path = $rootLevelSolution.FullName
        }
    }

    if ($PreferSolution) {
        $firstSolution = Get-FirstMatch -BasePath $BasePath -Filter "*.sln"
        if ($firstSolution) {
            return @{
                Type = "solution"
                Path = $firstSolution
            }
        }
    }

    $srcPath = Join-Path $BasePath "src"
    if (Test-Path -LiteralPath $srcPath) {
        $firstProject = Get-FirstMatch -BasePath $srcPath -Filter "*.csproj"
        if ($firstProject) {
            return @{
                Type = "project"
                Path = $firstProject
            }
        }
    }

    $fallbackSolution = Get-FirstMatch -BasePath $BasePath -Filter "*.sln"
    if ($fallbackSolution) {
        return @{
            Type = "solution"
            Path = $fallbackSolution
        }
    }

    $fallbackProject = Get-FirstMatch -BasePath $BasePath -Filter "*.csproj"
    if ($fallbackProject) {
        return @{
            Type = "project"
            Path = $fallbackProject
        }
    }

    throw "No .sln or .csproj file found under '$BasePath'."
}

function Resolve-TestTargets {
    param(
        [string]$BasePath,
        [string]$ExplicitTestProject
    )

    if ($ExplicitTestProject) {
        return @((Resolve-Path -LiteralPath $ExplicitTestProject).Path)
    }

    $testRoots = @(
        (Join-Path $BasePath "test"),
        (Join-Path $BasePath "tests")
    ) | Where-Object { Test-Path -LiteralPath $_ }

    $projects = foreach ($testRoot in $testRoots) {
        Get-ChildItem -LiteralPath $testRoot -Filter "*.csproj" -Recurse -File -ErrorAction SilentlyContinue
    }

    return @($projects | Sort-Object FullName | Select-Object -ExpandProperty FullName)
}

function Invoke-DotnetCommand {
    param(
        [string[]]$Arguments,
        [string]$WorkingDirectory
    )

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "dotnet"
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true

    $escapedArguments = foreach ($argument in $Arguments) {
        if ($argument -match '\s|"') {
            '"' + ($argument -replace '"', '\"') + '"'
        }
        else {
            $argument
        }
    }
    $psi.Arguments = ($escapedArguments -join " ")

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    [void]$process.Start()

    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()

    return @{
        ExitCode = $process.ExitCode
        StdOut = $stdout
        StdErr = $stderr
        Combined = (($stdout, $stderr) -join [Environment]::NewLine).Trim()
    }
}

function Get-ErrorLines {
    param(
        [string]$Text
    )

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return @()
    }

    return @(
        $Text -split "(`r`n|`n|`r)" |
        Where-Object {
            $_ -match ":\s+error\s+" -or
            $_ -match "^Build FAILED\.$" -or
            $_ -match "^\s*Failed!\s*$" -or
            $_ -match "^\s*Error:" -or
            $_ -match "Unhandled exception"
        } |
        Select-Object -First 30
    )
}

$resolvedRoot = (Resolve-Path -LiteralPath $Root).Path
$buildTarget = Resolve-BuildTarget -BasePath $resolvedRoot -SolutionPath $Solution -ProjectPath $Project -PreferSolution:$BuildSolution

$buildArgs = @("build", $buildTarget.Path)
$buildResult = Invoke-DotnetCommand -Arguments $buildArgs -WorkingDirectory $resolvedRoot
$buildSucceeded = $buildResult.ExitCode -eq 0

$testSummaries = @()
$testExecuted = $false
$testSucceeded = $null

if ($RunTests -and $buildSucceeded) {
    $testTargets = Resolve-TestTargets -BasePath $resolvedRoot -ExplicitTestProject $TestProject
    $testExecuted = $testTargets.Count -gt 0
    $testSucceeded = $true

    foreach ($target in $testTargets) {
        $result = Invoke-DotnetCommand -Arguments @("test", $target, "--no-build") -WorkingDirectory $resolvedRoot
        $passed = $result.ExitCode -eq 0
        if (-not $passed) {
            $testSucceeded = $false
        }

        $testSummaries += [pscustomobject]@{
            target = $target
            exitCode = $result.ExitCode
            succeeded = $passed
            errorLines = @(Get-ErrorLines -Text $result.Combined)
        }
    }
}

$summary = [pscustomobject]@{
    root = $resolvedRoot
    build = [pscustomobject]@{
        targetType = $buildTarget.Type
        targetPath = $buildTarget.Path
        command = "dotnet build `"$($buildTarget.Path)`""
        exitCode = $buildResult.ExitCode
        succeeded = $buildSucceeded
        errorLines = @(Get-ErrorLines -Text $buildResult.Combined)
    }
    tests = [pscustomobject]@{
        requested = [bool]$RunTests
        executed = $testExecuted
        succeeded = $testSucceeded
        targets = $testSummaries
    }
}

$summary | ConvertTo-Json -Depth 6
