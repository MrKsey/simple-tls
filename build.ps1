# Build script for simple-tls
# Usage: .\build.ps1 [target]
# Targets: all, windows-amd64, windows-arm64, linux-amd64, linux-arm64, linux-mipsle-softfloat, etc.

$ErrorActionPreference = "Stop"

$VERSION = "1.0.0"

# Define all build targets
$targets = @{
    "windows-amd64" = @{OS="windows"; Arch="amd64"; MIPS=""; Ext="exe"}
    "windows-arm64" = @{OS="windows"; Arch="arm64"; MIPS=""; Ext="exe"}
    "linux-amd64" = @{OS="linux"; Arch="amd64"; MIPS=""; Ext=""}
    "linux-arm64" = @{OS="linux"; Arch="arm64"; MIPS=""; Ext=""}
    "linux-mipsle-softfloat" = @{OS="linux"; Arch="mipsle"; MIPS="softfloat"; Ext=""}
    "linux-mipsle-float" = @{OS="linux"; Arch="mipsle"; MIPS="float"; Ext=""}
    "linux-mips-softfloat" = @{OS="linux"; Arch="mips"; MIPS="softfloat"; Ext=""}
    "linux-mips-float" = @{OS="linux"; Arch="mips"; MIPS="float"; Ext=""}
    "linux-mips64" = @{OS="linux"; Arch="mips64"; MIPS=""; Ext=""}
}

function Build($targetName) {
    $target = $targets[$targetName]
    if (!$target) {
        Write-Host "Unknown target: $targetName" -ForegroundColor Red
        return $false
    }
    
    Write-Host "Building $targetName..." -ForegroundColor Cyan
    
    $env:GOOS = $target.OS
    $env:GOARCH = $target.Arch
    if ($target.MIPS) {
        $env:GOMIPS = $target.MIPS
    }
    
    $outputName = "simple-tls-$targetName"
    if ($target.Ext) {
        $outputName += ".$($target.Ext)"
    }
    
    $ldflags = "-s -w -X main.version=$VERSION"
    
    & "C:\Program Files\Go\bin\go.exe" build -ldflags="$ldflags" -o "build/$outputName" .
    
    if ($LASTEXITCODE -eq 0) {
        $sizeMB = [math]::Round((Get-Item "build/$outputName").Length / 1MB, 2)
        Write-Host "  ✓ Built: $outputName ($sizeMB MB)" -ForegroundColor Green
        $true
    } else {
        Write-Host "  ✗ Failed to build $targetName" -ForegroundColor Red
        $false
    }
    
    # Clear environment
    Remove-Item Env:\GOOS -ErrorAction SilentlyContinue
    Remove-Item Env:\GOARCH -ErrorAction SilentlyContinue
    Remove-Item Env:\GOMIPS -ErrorAction SilentlyContinue
}

# Create build directory
if (!(Test-Path "build")) {
    New-Item -ItemType Directory -Path "build" | Out-Null
}

# Get target from argument or build all
$target = $args[0]

if (!$target -or $target -eq "all") {
    Write-Host "Building all targets..." -ForegroundColor Yellow
    Write-Host ""
    
    $success = $true
    foreach ($targetName in $targets.Keys) {
        if (!(Build $targetName)) {
            $success = $false
        }
    }
    
    if ($success) {
        Write-Host "`n✓ All builds completed successfully!" -ForegroundColor Green
        Write-Host "Output directory: build/" -ForegroundColor Cyan
        Write-Host ""
        Get-ChildItem "build" | Format-Table Name, @{Label="Size(MB)";Expression={[math]::Round($_.Length/1MB,2)}} -AutoSize
    } else {
        Write-Host "`n✗ Some builds failed" -ForegroundColor Red
        exit 1
    }
    
} elseif ($targets.ContainsKey($target)) {
    if (!(Build $target)) {
        exit 1
    }
} else {
    Write-Host "Unknown target: $target" -ForegroundColor Red
    Write-Host "Usage: .\build.ps1 [all|windows-amd64|windows-arm64|linux-amd64|linux-arm64|linux-mipsle-softfloat|...]" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Available targets:" -ForegroundColor Cyan
    $targets.Keys | Sort-Object | ForEach-Object { Write-Host "  $_" }
    exit 1
}

