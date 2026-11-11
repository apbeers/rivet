"""Exercise real AppKit controls with an explicit in-memory SDK fixture."""
import pathlib
import subprocess
import sys


def main():
    command = [str(pathlib.Path('target/debug/rivet').resolve()), 'ui', '--live-smoke']
    log = pathlib.Path('rivet-live-smoke.log')
    with log.open('w') as output:
        child = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, text=True)
        try:
            code = child.wait(timeout=90)
        except subprocess.TimeoutExpired:
            subprocess.run(['sample', str(child.pid), '3', '-file', 'rivet-live-sample.txt'], check=False, timeout=15)
            child.kill()
            child.wait(timeout=5)
            code = -1
    text = log.read_text()
    print(text)
    return 0 if code == 0 and 'RIVET_LIVE_UI_SMOKE_PASS' in text else 1


if __name__ == '__main__':
    sys.exit(main())
