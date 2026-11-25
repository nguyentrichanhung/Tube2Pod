param([string]$Config = (Join-Path $PSScriptRoot 'tube2pod.env'))
$Config = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Config)
Set-Location -LiteralPath $PSScriptRoot
& (Join-Path $PSScriptRoot 'tube2pod.exe') --config $Config
exit $LASTEXITCODE
