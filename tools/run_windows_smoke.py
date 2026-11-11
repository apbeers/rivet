"""Actual Win32/ConPTY/WebView2 test; the agent transport alone is an explicit fixture."""
import os
import pathlib
import subprocess
import sys


def main():
    if sys.platform != 'win32':
        raise SystemExit('Requires a real Windows desktop session; no headless simulation')
    binary = pathlib.Path(os.environ.get('RIVET_WINDOWS_BINARY', 'target/debug/rivet.exe')).resolve()
    with pathlib.Path('rivet-windows-smoke.log').open('w') as log:
        child = subprocess.Popen([str(binary), 'ui', '--smoke'], stdout=log, stderr=subprocess.STDOUT, text=True)
        try:
            code = child.wait(timeout=120)
        except subprocess.TimeoutExpired:
            subprocess.run(['taskkill', '/PID', str(child.pid), '/T', '/F'], capture_output=True, check=False)
            child.wait(timeout=10)
            code = -1
    text = pathlib.Path('rivet-windows-smoke.log').read_text(encoding='utf-8', errors='replace')
    print(text)
    if code != 0 or 'RIVET_WINDOWS_UI_SMOKE_PASS' not in text:
        return 1
    for name in ('rivet-windows-agents.bmp', 'rivet-windows-tools.bmp'):
        if not pathlib.Path(name).is_file():
            raise SystemExit(f'Missing native screenshot: {name}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
