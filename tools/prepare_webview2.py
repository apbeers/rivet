"""Acquire Microsoft's pinned native loader, not a Rust wrapper or browser engine."""
import hashlib
import io
import pathlib
import urllib.request
import zipfile

VERSION = '1.0.4314.35'
SHA256 = '90778deb1b77e36292828b4b5833c9255c27533f40e0ddd55891ae718944edc4'
URL = f'https://api.nuget.org/v3-flatcontainer/microsoft.web.webview2/{VERSION}/microsoft.web.webview2.{VERSION}.nupkg'


def prepare(destination):
    data = urllib.request.urlopen(URL, timeout=90).read()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise RuntimeError('WebView2 package checksum mismatch')
    archive = zipfile.ZipFile(io.BytesIO(data))
    destination.mkdir(parents=True, exist_ok=True)
    (destination / 'WebView2Loader.dll').write_bytes(archive.read('build/native/x64/WebView2Loader.dll'))
    licenses = [n for n in archive.namelist() if pathlib.PurePosixPath(n).name.lower() in ('license.txt', 'license', 'license.md')]
    if not licenses:
        raise RuntimeError('Microsoft package license missing')
    (destination / 'WebView2-LICENSE.txt').write_bytes(archive.read(licenses[0]))
    print(f'Installed pinned Microsoft WebView2 loader {VERSION} in {destination}')


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('destination', type=pathlib.Path)
    prepare(parser.parse_args().destination)
