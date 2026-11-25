param(
    [Parameter(Mandatory=$true)][ValidateSet('x86_64-pc-windows-msvc')][string]$Target,
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$OutDir
)
$ErrorActionPreference = 'Stop'
if (!(Test-Path -LiteralPath $Binary -PathType Leaf)) { throw "Binary not found: $Binary" }
$Binary = (Resolve-Path -LiteralPath $Binary).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
$name = "tube2pod-$Target"
try {
    $package = New-Item -ItemType Directory -Path (Join-Path $stage $name)
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $package.FullName 'tube2pod.exe')
    foreach ($file in @('start.ps1','portable.env.example','Caddyfile.example','portable.md','direct-https.md')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $package.FullName
    }
    $archive = Join-Path $OutDir "$name.zip"
    Compress-Archive -LiteralPath $package.FullName -DestinationPath $archive -Force
    Write-Output $archive
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
