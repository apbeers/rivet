#!/bin/bash
# Install/update a verified local development bundle without elevation.
set -euo pipefail
if [[ "$(uname -s)" != Darwin || $# != 1 || ! -d "$1/Contents/MacOS" ]]; then
  echo 'Usage (macOS): bash tools/install_macos.sh /path/to/Rivet.app' >&2
  exit 2
fi
source_bundle="$1"
destination="$HOME/Applications/Rivet.app"
if pgrep -f "$destination/Contents/MacOS/rivet" >/dev/null; then
  echo 'Quit Rivet before updating it.' >&2
  exit 1
fi
codesign --verify --deep --strict "$source_bundle"
mkdir -p "$HOME/Applications"
staging="$(mktemp -d "$HOME/Applications/.rivet-install.XXXXXX")"
restore_backup() {
  if [[ ! -e "$destination" && -d "$staging/Previous.app" ]]; then
    mv "$staging/Previous.app" "$destination"
  fi
  rm -rf "$staging"
}
trap restore_backup EXIT
# Fully copy and validate before touching an existing installation.
ditto "$source_bundle" "$staging/Rivet.app"
codesign --verify --deep --strict "$staging/Rivet.app"
if [[ -e "$destination" ]]; then mv "$destination" "$staging/Previous.app"; fi
mv "$staging/Rivet.app" "$destination"
echo "Installed $destination. Development signing does not establish Gatekeeper distribution trust."
