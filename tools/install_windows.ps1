param([string]$Source = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
$Source = (Resolve-Path $Source).Path
$Parent = Join-Path $env:LOCALAPPDATA 'Programs'
$Destination = Join-Path $Parent 'Rivet'
$Required = @('Rivet.exe', 'WebView2Loader.dll', 'WebView2-LICENSE.txt', 'README.txt', 'Install-Rivet.ps1', 'Uninstall-Rivet.ps1')
foreach ($Name in $Required) { if (-not (Test-Path (Join-Path $Source $Name) -PathType Leaf)) { throw "Incomplete package: $Name" } }
Get-Process -Name Rivet -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $Destination 'Rivet.exe') } | ForEach-Object { throw 'Quit Rivet before updating it' }
New-Item -ItemType Directory -Path $Parent -Force | Out-Null
$Staging = Join-Path $Parent ('.rivet-install-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $Staging | Out-Null
try {
    $New = Join-Path $Staging 'New'
    New-Item -ItemType Directory -Path $New | Out-Null
    foreach ($Name in $Required) { Copy-Item (Join-Path $Source $Name) (Join-Path $New $Name) }
    if (Test-Path $Destination) { Move-Item $Destination (Join-Path $Staging 'Previous') }
    Move-Item $New $Destination
    Write-Output "Installed $Destination (current user; no elevation requested)."
} finally {
    if (-not (Test-Path $Destination) -and (Test-Path (Join-Path $Staging 'Previous'))) { Move-Item (Join-Path $Staging 'Previous') $Destination }
    Remove-Item $Staging -Recurse -Force
}
