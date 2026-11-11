"""CI-only browser prerequisite. Product install never silently installs dependencies."""
import pathlib
import subprocess
import tempfile
import urllib.request

binary = str(pathlib.Path('target/debug/rivet.exe').resolve())
probe = lambda: subprocess.run([binary, 'webview-version'], capture_output=True, text=True, timeout=20)
result = probe()
if result.returncode:
    with tempfile.TemporaryDirectory(prefix='rivet-webview-ci-') as temporary:
        installer = pathlib.Path(temporary) / 'MicrosoftEdgeWebview2Setup.exe'
        installer.write_bytes(urllib.request.urlopen('https://go.microsoft.com/fwlink/p/?LinkId=2124703', timeout=90).read())
        # Microsoft chooses user/machine scope from the current token. No elevation is requested.
        subprocess.run([str(installer), '/silent', '/install'], check=True, timeout=180)
    result = probe()
print(result.stdout)
if result.returncode:
    raise SystemExit(f'WebView2 CI prerequisite unavailable: {result.stderr}')
