#Requires -Version 5.1
[CmdletBinding()]
param(
    [Alias('skip-build')]
    [switch]$SkipBuild,
    [Alias('skip-landstrip')]
    [switch]$SkipLandstrip,
    [Alias('update-resources')]
    [switch]$UpdateResources
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

try {
    $ProjectRoot = $PSScriptRoot
    if (-not $SkipBuild) {
        if (-not (Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue)) {
            throw 'Install the Rust toolchain first.'
        }
        Write-Host 'Building Margatroid...'
        Push-Location -LiteralPath $ProjectRoot
        try {
            & cargo build --locked --release -p margatroid -p tool_plugin -p shell-client -p mcp-client --bins
            if ($LASTEXITCODE -ne 0) {
                throw "Build failed (exit $LASTEXITCODE)."
            }
        }
        finally {
            Pop-Location
        }
    }

    $Installer = Join-Path $ProjectRoot 'target/release/margatroid-install.exe'
    if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
        throw 'Build the installation helper first, or omit -SkipBuild.'
    }
    $InstallerArguments = @($ProjectRoot)
    if ($SkipLandstrip) { $InstallerArguments += '--skip-landstrip' }
    if ($UpdateResources) { $InstallerArguments += '--update-resources' }
    & $Installer @InstallerArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Installation failed (exit $LASTEXITCODE)."
    }
}
catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
