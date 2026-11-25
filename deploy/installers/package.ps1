param([Parameter(Mandatory=$true)][string]$Binary, [Parameter(Mandatory=$true)][string]$OutDir)
$ErrorActionPreference = 'Stop'
$binaryPath = (Resolve-Path $Binary).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$outputPath = (Resolve-Path $OutDir).Path
& "${env:ProgramFiles(x86)}/Inno Setup 6/ISCC.exe" "/DBinary=$binaryPath" "/DOutputDir=$outputPath" "$PSScriptRoot/tube2pod.iss"
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
