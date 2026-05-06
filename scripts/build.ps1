# Example:
# ./scripts/build.ps1 -Backend vulkan
#
# This builds the Sena project from the sena/ directory and auto-detects the
# installed Vulkan SDK under C:\VulkanSDK if VULKAN_SDK is not already set.

param(
    [ValidateSet('auto', 'vulkan', 'cuda', 'metal', 'llama', 'mock', 'none')]
    [string]$Backend = 'auto',

    [ValidateSet('debug', 'release')]
    [string]$Configuration = 'debug',

    [string]$WorkspaceRoot = (Resolve-Path -Path "$(Split-Path -Parent $MyInvocation.MyCommand.Path)\..\sena"),

    [switch]$Verbose
)

function Write-Info {
    param([string]$Message)
    Write-Host "[build] $Message"
}

function Fail {
    param([string]$Message)
    throw "[build] $Message"
}

function Find-LatestVulkanSdk {
    if ($env:VULKAN_SDK) {
        return $env:VULKAN_SDK
    }

    $root = 'C:\VulkanSDK'
    if (-not (Test-Path $root)) {
        return $null
    }

    $sdks = Get-ChildItem -Path $root -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^[0-9]+\.[0-9]+' } |
        Sort-Object Name -Descending

    if ($sdks) {
        return $sdks[0].FullName
    }

    return $null
}

function Get-NinjaCandidatePaths {
    $candidates = @(
        "$env:VULKAN_SDK\Bin\ninja.exe",
        'C:\Program Files\CMake\bin\ninja.exe',
        'C:\Program Files (x86)\CMake\bin\ninja.exe',
        'C:\Program Files\Ninja\ninja.exe',
        'C:\Program Files (x86)\Ninja\ninja.exe',
        'C:\ProgramData\chocolatey\bin\ninja.exe',
        "$env:LOCALAPPDATA\Microsoft\WinGet\Links\ninja.exe",
        'C:\msys64\usr\bin\ninja.exe'
    )

    $visualStudioPatterns = @(
        'C:\Program Files\Microsoft Visual Studio\*\*\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe',
        'C:\Program Files (x86)\Microsoft Visual Studio\*\*\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
    )

    foreach ($pattern in $visualStudioPatterns) {
        $candidates += Get-ChildItem -Path $pattern -File -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending |
            Select-Object -ExpandProperty FullName
    }

    return $candidates |
        Where-Object { $_ } |
        Select-Object -Unique
}

function Find-NinjaExecutable {
    $command = Get-Command ninja -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    foreach ($candidate in Get-NinjaCandidatePaths) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    return $null
}

function Test-GlslcInstalled {
    if (Get-Command glslc -ErrorAction SilentlyContinue) {
        return $true
    }
    return $false
}

function Set-VulkanEnvironment {
    $sdk = Find-LatestVulkanSdk
    if (-not $sdk) {
        Fail 'VULKAN_SDK is not set and no Vulkan SDK was found under C:\VulkanSDK. Install the LunarG SDK or set VULKAN_SDK manually.'
    }

    Write-Info "Using Vulkan SDK: $sdk"
    $env:VULKAN_SDK = $sdk
    $env:PATH = "$sdk\Bin;$env:PATH"

    $ninjaPath = Find-NinjaExecutable
    if (-not $ninjaPath) {
        $searchedPaths = Get-NinjaCandidatePaths
        Fail "Ninja is required for Vulkan builds but was not found. Searched PATH and common locations: $($searchedPaths -join '; ')"
    }

    $ninjaDir = Split-Path -Parent $ninjaPath
    $pathEntries = $env:PATH -split ';'
    if ($pathEntries -notcontains $ninjaDir) {
        $env:PATH = "$ninjaDir;$env:PATH"
    }
    Write-Info "Using Ninja: $ninjaPath"

    if (-not (Test-GlslcInstalled)) {
        Fail 'glslc was not found on PATH. Ensure %VULKAN_SDK%\Bin is available or install the Vulkan SDK properly.'
    }

    $env:CMAKE_GENERATOR = 'Ninja'
    Write-Info 'Configured Vulkan build environment (Ninja + glslc).'
}

function Invoke-WorkspaceBuild {
    param(
        [string]$FeatureArg
    )

    $originalTargetDir = $env:CARGO_TARGET_DIR
    $arguments = @('build', '--workspace')
    if ($FeatureArg) {
        $arguments += @('--features', $FeatureArg)
    }

    if ($Configuration -eq 'release') {
        $arguments += '--release'
    }

    if ($Verbose -or $FeatureArg -eq 'vulkan') {
        $arguments += '-v'
    }

    $arguments += @('--color', 'always')

    $command = "cargo $($arguments -join ' ')"
    $logPath = Join-Path $WorkspaceRoot 'build_log.txt'

    if (Test-Path $logPath) {
        Remove-Item $logPath -Force
    }

    $env:CARGO_TERM_PROGRESS_WHEN = 'never'
    $env:CARGO_TERM_COLOR = 'always'

    if ($FeatureArg -eq 'vulkan' -and -not $env:CARGO_TARGET_DIR) {
        $shortTargetDir = Join-Path ([System.IO.Path]::GetPathRoot($WorkspaceRoot)) 'sena-target-vulkan'
        $env:CARGO_TARGET_DIR = $shortTargetDir
        Write-Info "Using short target dir for Vulkan build: $shortTargetDir"
    }

    Write-Info "Running: $command"
    Write-Info "Logging build output to: $logPath"

    Push-Location $WorkspaceRoot
    try {
        & cargo @arguments 2>&1 |
            ForEach-Object {
                if ($_ -is [System.Management.Automation.ErrorRecord]) {
                    $_.ToString()
                }
                else {
                    "$_"
                }
            } |
            Tee-Object -FilePath $logPath
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
        if ($null -eq $originalTargetDir) {
            Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        }
        else {
            $env:CARGO_TARGET_DIR = $originalTargetDir
        }
    }

    if ($exitCode -ne 0) {
        Fail "cargo build failed with exit code $exitCode. See $logPath"
    }

    Write-Info 'Build completed successfully.'
}

try {
    Write-Info "Workspace root: $WorkspaceRoot"
    Set-Location $WorkspaceRoot

    $chosenBackend = $Backend
    if ($Backend -eq 'auto') {
        $vulkanExists = Find-LatestVulkanSdk
        if ($vulkanExists) {
            $chosenBackend = 'vulkan'
        }
        else {
            Write-Info 'No Vulkan SDK detected; defaulting to no backend features.'
            $chosenBackend = 'none'
        }
    }

    switch ($chosenBackend) {
        'vulkan' {
            Set-VulkanEnvironment
            Invoke-WorkspaceBuild -FeatureArg 'vulkan'
        }
        'cuda' {
            Invoke-WorkspaceBuild -FeatureArg 'cuda'
        }
        'metal' {
            Invoke-WorkspaceBuild -FeatureArg 'metal'
        }
        'llama' {
            Invoke-WorkspaceBuild -FeatureArg 'llama'
        }
        'mock' {
            Invoke-WorkspaceBuild -FeatureArg ''
        }
        'none' {
            Invoke-WorkspaceBuild -FeatureArg ''
        }
        default {
            Fail "Unknown backend '$Backend'. Valid values are auto, vulkan, cuda, metal, llama, mock, none."
        }
    }
}
catch {
    $message = $_.Exception.Message
    if (-not $message) {
        $message = "$_"
    }

    [Console]::Error.WriteLine($message)
    exit 1
}