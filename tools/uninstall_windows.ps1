$ErrorActionPreference = 'Stop'
$Destination = Join-Path $env:LOCALAPPDATA 'Programs/Rivet'
Get-Process -Name Rivet -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $Destination 'Rivet.exe') } | ForEach-Object { throw 'Quit Rivet before uninstalling it' }
# Remove only the known application files; leave user data and unknown files alone.
foreach ($Name in @('Rivet.exe', 'WebView2Loader.dll', 'WebView2-LICENSE.txt', 'README.txt', 'Install-Rivet.ps1', 'Uninstall-Rivet.ps1')) { Remove-Item (Join-Path $Destination $Name) -Force -ErrorAction SilentlyContinue }
if ((Test-Path $Destination) -and -not (Get-ChildItem $Destination -Force)) { Remove-Item $Destination }
Write-Output 'Rivet uninstalled for this user. Sessions/settings remain under LocalAppData/Rivet.'
