param([string]$Binary = 'target/debug/rivet.exe', [string]$Output = 'dist/windows')
$ErrorActionPreference = 'Stop'
if (Test-Path $Output) { throw "Refusing to overwrite existing package: $Output" }
New-Item -ItemType Directory -Path $Output -Force | Out-Null
Copy-Item $Binary (Join-Path $Output 'Rivet.exe')
Copy-Item 'tools/install_windows.ps1' (Join-Path $Output 'Install-Rivet.ps1')
Copy-Item 'tools/uninstall_windows.ps1' (Join-Path $Output 'Uninstall-Rivet.ps1')
$ManifestTool = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/mt.exe" | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $ManifestTool) { throw 'Windows SDK manifest tool not found' }
$Exe = (Resolve-Path (Join-Path $Output 'Rivet.exe')).Path
& $ManifestTool.FullName -nologo -manifest tools/windows.manifest "-outputresource:$Exe;#1"
if ($LASTEXITCODE -ne 0) { throw 'Failed to embed asInvoker manifest' }
python tools/prepare_webview2.py $Output
if ($LASTEXITCODE -ne 0) { throw 'WebView2 loader preparation failed' }
@'
Rivet Windows x64 development build. Windows 10 1809+ / Windows 11 required.
Extract the entire ZIP first, then double-click Rivet.exe. No installer is needed.
Keep the other extracted files beside Rivet.exe. Move the folder anywhere in your account.
Quit Rivet before replacing that folder with a newer download. Delete it to uninstall.
No Rust toolchain, administrator rights, service or PATH change is needed.
WebView2 Evergreen Runtime is required for embedded browsing. If absent, install
it for the current user from https://developer.microsoft.com/microsoft-edge/webview2/.
Git and language servers are optional installed development tools.
Authenticate using copilot login through the supported Copilot CLI before agent use.
This development build is unsigned; Authenticode/SmartScreen and fresh standard-user
release acceptance remain open. Terminal emulation is constrained. See the repository
https://github.com/apbeers/rivet#download-and-run for setup and limits.
No OS filesystem sandbox is claimed.
'@ | Set-Content (Join-Path $Output 'README.txt')
Compress-Archive -Path "$Output/*" -DestinationPath 'dist/Rivet-Windows-x64-development.zip'
