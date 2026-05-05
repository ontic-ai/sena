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
    Write-Error "[build] $Message"
    exit 1
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

function Test-NinjaInstalled {
    if (Get-Command ninja -ErrorAction SilentlyContinue) {
        return $true
    }
    return $false
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

    if (-not (Test-NinjaInstalled)) {
        Fail 'Ninja is required for Vulkan builds but was not found on PATH.'
    }

    if (-not (Test-GlslcInstalled)) {
        Fail 'glslc was not found on PATH. Ensure %VULKAN_SDK%\Bin is available or install the Vulkan SDK properly.'
    }

    $env:CMAKE_GENERATOR = 'Ninja'
    Write-Info 'Configured Vulkan build environment (Ninja + glslc).'
}

function Build-Workspace {
    param(
        [string]$FeatureArg
    )

    $featureFlag = ''
    if ($FeatureArg) {
        $featureFlag = "--features $FeatureArg"
    }

    $configFlag = ''
    if ($Configuration -eq 'release') {
        $configFlag = '--release'
    }

    $verboseFlag = ''
    if ($Verbose -or $FeatureArg -eq 'vulkan') {
        $verboseFlag = '-v'
    }

    $command = "cargo build --workspace $featureFlag $configFlag $verboseFlag --color always"
    Write-Info "Running: $command"

    $processInfo = New-Object System.Diagnostics.ProcessStartInfo
    $processInfo.FileName = 'cargo'
    $processInfo.Arguments = "build --workspace $featureFlag $configFlag $verboseFlag --color always"
    $processInfo.WorkingDirectory = $WorkspaceRoot
    $processInfo.RedirectStandardOutput = $true
    $processInfo.RedirectStandardError = $true
    $processInfo.UseShellExecute = $false
    $processInfo.EnvironmentVariables['VULKAN_SDK'] = $env:VULKAN_SDK
    $processInfo.EnvironmentVariables['CMAKE_GENERATOR'] = $env:CMAKE_GENERATOR
        $processInfo.EnvironmentVariables['CARGO_TERM_PROGRESS_WHEN'] = 'never'
    $processInfo.EnvironmentVariables['CARGO_TERM_COLOR'] = 'always'

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $processInfo
    $process.Start() | Out-Null

    while (-not $process.HasExited) {
        $stdout = $process.StandardOutput.ReadLine()
        if ($null -ne $stdout) { Write-Host $stdout }
    }

    $stderr = $process.StandardError.ReadToEnd()
    if ($stderr) { Write-Host $stderr }

    if ($process.ExitCode -ne 0) {
        Fail "cargo build failed with exit code $($process.ExitCode)"
    }

    Write-Info 'Build completed successfully.'
}

# Main execution
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
        Build-Workspace -FeatureArg 'vulkan'
    }
    'cuda' {
        Build-Workspace -FeatureArg 'cuda'
    }
    'metal' {
        Build-Workspace -FeatureArg 'metal'
    }
    'llama' {
        Build-Workspace -FeatureArg 'llama'
    }
    'mock' {
        Build-Workspace -FeatureArg ''
    }
    'none' {
        Build-Workspace -FeatureArg ''
    }
    default {
        Fail "Unknown backend '$Backend'. Valid values are auto, vulkan, cuda, metal, llama, mock, none."
    }
}