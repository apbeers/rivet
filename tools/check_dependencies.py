#!/usr/bin/env python3
"""Enforce direct dependency exceptions; no third-party Python packages needed."""
import json
import subprocess


def check(metadata):
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    paths = {p["name"] for p in packages}
    errors = []
    for package in packages:
        for dep in package["dependencies"]:
            if dep["source"] is None and dep["name"] in paths:
                continue
            if package["name"] == "rivet-copilot" and dep["name"] in {"github-copilot-sdk", "tokio"}:
                continue
            if (package["name"] == "rivet-macos"
                    and dep["name"] in {"objc2", "objc2-foundation", "objc2-app-kit"}
                    and dep.get("target") == 'cfg(target_os = "macos")'
                    and dep.get("kind") is None):
                continue
            errors.append(f'{package["name"]}: forbidden direct dependency {dep["name"]}')
    return errors


if __name__ == "__main__":
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"
    ], text=True))
    errors = check(metadata)
    if errors:
        raise SystemExit("\n".join(errors))
    print("Dependency policy passed: SDK/Tokio adapter + macOS-only native bindings.")
