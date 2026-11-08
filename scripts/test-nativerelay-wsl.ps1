<# 
.SYNOPSIS
    Runs the NativeRelay Linux integration test in WSL2 Ubuntu from Windows.
.DESCRIPTION
    This script builds AgentTrace, installs NativeRelay in WSL Ubuntu, and runs
    the Linux integration test script with root privileges.
.NOTES
    Requires: WSL2 with Ubuntu installed, Rust toolchain, Python3/pip in WSL
#>

param(
    [switch]$SkipBuild,
    [switch]$SkipNativeRelayInstall
)

$ErrorActionPreference = "Stop"
$projectPath = "C:\Users\DAVID UZOMAH\Desktop\LogicLabs\AgentTrace"
$wslProjectPath = "/mnt/c/Users/DAVID UZOMAH/Desktop/LogicLabs/AgentTrace"

Write-Host "=== NativeRelay Linux Integration Test via WSL2 ===" -ForegroundColor Cyan
Write-Host "Project: $projectPath" -ForegroundColor Gray

# Check WSL is available
if (-not (Get-Command wsl -ErrorAction SilentlyContinue)) {
    Write-Error "WSL not found. Enable WSL2 first: wsl --install -d Ubuntu"
    exit 1
}

# Check Ubuntu is installed
$p = New-Object System.Diagnostics.Process
$p.StartInfo.FileName = 'wsl.exe'
$p.StartInfo.Arguments = '-l -q'
$p.StartInfo.RedirectStandardOutput = $true
$p.StartInfo.UseShellExecute = $false
$p.StartInfo.StandardOutputEncoding = [System.Text.Encoding]::Unicode
$p.Start() | Out-Null
$distrosRaw = $p.StandardOutput.ReadToEnd()
$p.WaitForExit()

$distros = $distrosRaw -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notmatch '^\s*$' }
Write-Host "    Detected WSL distros: $($distros -join ', ')" -ForegroundColor Gray
if ($distros -notcontains "Ubuntu") {
    Write-Error "Ubuntu not found in WSL. Available: $($distros -join ', '). Install: wsl --install -d Ubuntu"
    exit 1
}

Write-Host "`n[1/5] Building AgentTrace (Windows)..." -ForegroundColor Yellow
if (-not $SkipBuild) {
    Push-Location $projectPath
    cargo build --locked
    Pop-Location
    Write-Host "    Build complete" -ForegroundColor Green
} else {
    Write-Host "    Skipped (--SkipBuild)" -ForegroundColor Gray
}

Write-Host "`n[2/5] Setting up WSL environment..." -ForegroundColor Yellow
$setupScript = @'
set -euo pipefail
echo "Updating apt..."
sudo apt-get update -qq
echo "Installing python3/pip..."
sudo apt-get install -y -qq python3 python3-pip
if ! command -v nativerelay &> /dev/null; then
    echo "Installing NativeRelay 0.2.0..."
    pip3 install nativerelay==0.2.0
else
    echo "NativeRelay already installed"
fi
echo "Setup complete"
'@

if (-not $SkipNativeRelayInstall) {
    wsl -d Ubuntu -e bash -c $setupScript
    Write-Host "    WSL setup complete" -ForegroundColor Green
} else {
    Write-Host "    Skipped (--SkipNativeRelayInstall)" -ForegroundColor Gray
}

Write-Host "`n[3/5] Running Linux integration test (requires sudo)..." -ForegroundColor Yellow
Write-Host "    You may be prompted for your WSL sudo password" -ForegroundColor Gray

# Properly escape the path for bash (handle spaces)
$escapedPath = $wslProjectPath -replace ' ', '\ '
$testScript = 'set -euo pipefail; cd ' + $escapedPath + '; export PATH="$PATH:/home/$USER/.local/bin"; bash scripts/test-nativerelay-linux.sh'

wsl -d Ubuntu -e bash -c $testScript

Write-Host "`n[4/5] Test completed successfully!" -ForegroundColor Green
Write-Host "`n=== Summary ===" -ForegroundColor Cyan
Write-Host "AgentTrace binary: target/debug/agenttrace" -ForegroundColor Gray
Write-Host "Test script: scripts/test-nativerelay-linux.sh" -ForegroundColor Gray
Write-Host "Run again with -SkipBuild -SkipNativeRelayInstall for faster iteration" -ForegroundColor Gray