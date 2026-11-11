"""Create a per-user-installable development app bundle. Does not claim notarization."""
import argparse
import pathlib
import plistlib
import shutil
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/release/rivet')
    parser.add_argument('--output', default='dist')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('AppKit bundles must be assembled and verified on macOS')
    binary = pathlib.Path(args.binary).resolve()
    bundle = pathlib.Path(args.output).resolve() / 'Rivet.app'
    if bundle.exists():
        raise SystemExit(f'Refusing to overwrite an existing bundle: {bundle}')
    executable = bundle / 'Contents/MacOS/rivet'
    executable.parent.mkdir(parents=True)
    shutil.copy2(binary, executable)
    executable.chmod(0o755)
    info = dict(CFBundleIdentifier='com.apbeers.rivet', CFBundleName='Rivet',
                CFBundleDisplayName='Rivet', CFBundleExecutable='rivet',
                CFBundlePackageType='APPL', CFBundleShortVersionString='0.1.0',
                CFBundleVersion='1', LSMinimumSystemVersion='13.0', NSHighResolutionCapable=True)
    with (bundle / 'Contents/Info.plist').open('wb') as output:
        plistlib.dump(info, output)
    subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
    package = bundle.parent / 'macos'
    package.mkdir()
    subprocess.run(['ditto', str(bundle), str(package / 'Rivet.app')], check=True)
    (package / 'README.txt').write_text(
        'Rivet preview for macOS (Apple Silicon).\n\n'
        'Unzip the download and double-click Rivet.app. No installer is needed.\n'
        'Optionally copy Rivet.app into ~/Applications for a permanent home.\n'
        'Quit Rivet before replacing it with a newer download. Remove the app to uninstall.\n'
        'No Rust toolchain or administrator rights are needed for this per-user install.\n'
        'This development build is ad-hoc signed, not notarized. If macOS blocks it, see:\n'
        'https://support.apple.com/en-us/102445\n\n'
        'Sign in through Copilot CLI using copilot login before agent use.\n'
        'Setup, prerequisites and current limits: https://github.com/apbeers/rivet#download-and-run\n',
        encoding='utf-8')
    # Ditto preserves bundle metadata. Ad-hoc signing is for development, not distribution trust.
    archive = bundle.parent / 'Rivet-macOS-development.zip'
    subprocess.run(['ditto', '-c', '-k', '--sequesterRsrc', str(package), str(archive)], check=True)
    print(f'{archive}\nDevelopment bundle only: Developer ID signing, notarization, and standard-user release acceptance remain required.')


if __name__ == '__main__':
    main()
