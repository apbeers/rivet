"""Publish validated native CI packages to a rolling, explicitly marked prerelease."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import struct
import subprocess
import zipfile

TAG = 'preview'
MARKER = '<!-- rivet-managed-preview -->'
WINDOWS_FILES = ('Rivet.exe', 'WebView2Loader.dll', 'WebView2-LICENSE.txt', 'README.txt',
                 'Install-Rivet.ps1', 'Uninstall-Rivet.ps1')
MAC_FILES = ('Rivet.app/Contents/MacOS/rivet', 'Rivet.app/Contents/Info.plist',
             'README.txt')


def check_package(path, platform):
    with zipfile.ZipFile(path) as archive:
        required = WINDOWS_FILES if platform == 'windows' else MAC_FILES
        missing = set(required) - set(archive.namelist())
        if missing:
            raise ValueError(f'{path}: incomplete package: {sorted(missing)}')
        with archive.open(required[0]) as binary:
            if platform == 'windows':
                dos = binary.read(64)
                if len(dos) != 64 or dos[:2] != b'MZ':
                    raise ValueError('Windows executable has no DOS header')
                offset = struct.unpack_from('<I', dos, 60)[0]
                if not 64 <= offset <= 1024 * 1024:
                    raise ValueError('Windows PE header is outside its budget')
                binary.seek(offset)
                if binary.read(6) != b'PE\0\0\x64\x86':
                    raise ValueError('Windows download must contain an x64 executable')
            elif binary.read(8) != struct.pack('<II', 0xfeedfacf, 0x0100000c):
                raise ValueError('macOS download must contain an Apple Silicon executable')


def prepare(artifacts, output, repo, sha, run_id):
    if not re.fullmatch(r'[\w.-]+/[\w.-]+', repo) or not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('Expected a repository and full source commit SHA')
    if not re.fullmatch(r'[0-9]+', run_id):
        raise ValueError('Expected a numeric CI run ID')
    packages = (
        (artifacts / 'windows/Rivet-Windows-x64-development.zip', 'Rivet-Windows-x64.zip', 'windows'),
        (artifacts / 'macos/Rivet-macOS-development.zip', 'Rivet-macOS-arm64.zip', 'macos'),
    )
    # Validate both before producing any publishable payload.
    for source, _, platform in packages:
        check_package(source, platform)
    output.mkdir(parents=True, exist_ok=False)
    for source, name, _ in packages:
        shutil.copyfile(source, output / name)
    (output / 'BUILD-INFO.txt').write_text(
        f'Rivet development preview\nCommit: {sha}\n'
        f'Source: https://github.com/{repo}/commit/{sha}\n'
        f'Checks: https://github.com/{repo}/actions/runs/{run_id}\n'
        'Profile: debug (development; performance acceptance remains open)\n'
        'Platforms: Windows x64; macOS Apple Silicon (arm64)\n', encoding='utf-8')
    checksums = []
    for path in sorted(output.iterdir()):
        with path.open('rb') as source:
            digest = hashlib.file_digest(source, 'sha256').hexdigest()
        checksums.append(f'{digest}  {path.name}\n')
    (output / 'SHA256SUMS.txt').write_text(''.join(checksums), encoding='utf-8')


def api(path, method='GET', data=None, missing_ok=False):
    command = ['gh', 'api', path, '--method', method]
    if data is not None:
        command += ['--input', '-']
    result = subprocess.run(command, input=json.dumps(data) if data is not None else None,
                            capture_output=True, text=True, check=False)
    if result.returncode:
        if missing_ok and '(HTTP 404)' in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout)


def verify_output(output, repo, sha, run_id):
    names = {'Rivet-Windows-x64.zip', 'Rivet-macOS-arm64.zip', 'BUILD-INFO.txt'}
    if {p.name for p in output.iterdir()} != names | {'SHA256SUMS.txt'}:
        raise ValueError('Unexpected or missing release payload')
    seen = set()
    for line in (output / 'SHA256SUMS.txt').read_text(encoding='utf-8').splitlines():
        digest, separator, name = line.partition('  ')
        if not separator or name not in names or name in seen or not re.fullmatch('[0-9a-f]{64}', digest):
            raise ValueError('Invalid release checksum manifest')
        with (output / name).open('rb') as source:
            if hashlib.file_digest(source, 'sha256').hexdigest() != digest:
                raise ValueError(f'Release checksum mismatch: {name}')
        seen.add(name)
    if seen != names:
        raise ValueError('Incomplete release checksum manifest')
    info = (output / 'BUILD-INFO.txt').read_text(encoding='utf-8').splitlines()
    if (f'Commit: {sha}' not in info
            or f'Checks: https://github.com/{repo}/actions/runs/{run_id}' not in info):
        raise ValueError('Release provenance does not match this CI run')
    check_package(output / 'Rivet-Windows-x64.zip', 'windows')
    check_package(output / 'Rivet-macOS-arm64.zip', 'macos')


def publish(output, repo, sha, run_id):
    if os.environ.get('GITHUB_REF') != 'refs/heads/main':
        raise ValueError('Preview publication requires the main-branch CI job')
    verify_output(output, repo, sha, run_id)
    prefix = f'repos/{repo}'
    if api(f'{prefix}/git/ref/heads/main')['object']['sha'] != sha:
        print('Skipping superseded build; a newer main commit will publish after passing checks.')
        return
    release = api(f'{prefix}/releases/tags/{TAG}', missing_ok=True)
    if release and (MARKER not in (release.get('body') or '') or not release['prerelease']):
        raise ValueError('Refusing to replace a release not owned by the preview publisher')
    if release is None and api(f'{prefix}/git/ref/tags/{TAG}', missing_ok=True):
        raise ValueError('Refusing to reuse an existing unmanaged preview tag')
    notes = pathlib.Path(__file__).resolve().parents[1] / 'docs/preview-release.md'
    body = (f'{MARKER}\n{notes.read_text(encoding="utf-8")}\n\n'
            f'Build: [{sha[:7]}](https://github.com/{repo}/commit/{sha}) · '
            f'[CI checks](https://github.com/{repo}/actions/runs/{run_id}).\n'
            'The `preview` tag and downloads track the most recently published passing main build.\n')
    fields = dict(tag_name=TAG, target_commitish=sha, name='Rivet Preview', body=body,
                  draft=True, prerelease=True, make_latest='false')
    if release is None:
        release = api(f'{prefix}/releases', 'POST', fields)
    command = ['gh', 'release', 'upload', TAG, '--repo', repo, '--clobber']
    payload = sorted(p for p in output.iterdir() if p.name != 'SHA256SUMS.txt')
    subprocess.run(command + [str(p) for p in payload], check=True)
    # Expose new checksums only once all corresponding payloads have uploaded.
    subprocess.run(command + [str(output / 'SHA256SUMS.txt')], check=True)
    ref = api(f'{prefix}/git/ref/tags/{TAG}', missing_ok=True)
    if ref:
        api(f'{prefix}/git/refs/tags/{TAG}', 'PATCH', dict(sha=sha, force=True))
    else:
        api(f'{prefix}/git/refs', 'POST', dict(ref=f'refs/tags/{TAG}', sha=sha))
    fields['draft'] = False
    result = api(f'{prefix}/releases/{release["id"]}', 'PATCH', fields)
    print(f'Published {result["html_url"]} from {sha}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=pathlib.Path, default=pathlib.Path('artifacts'))
    parser.add_argument('--output', type=pathlib.Path, default=pathlib.Path('downloads'))
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--prepare-only', action='store_true')
    modes.add_argument('--publish-only', action='store_true')
    args = parser.parse_args()
    repo = os.environ['GITHUB_REPOSITORY']
    sha = os.environ['GITHUB_SHA']
    run_id = os.environ['GITHUB_RUN_ID']
    if args.prepare_only:
        prepare(args.artifacts, args.output, repo, sha, run_id)
    else:
        publish(args.output, repo, sha, run_id)


if __name__ == '__main__':
    main()
