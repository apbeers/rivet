# Rivet

A Rust-native workspace for AI agents, built for responsiveness and developer control.

Rivet will use native macOS and Windows interfaces over a shared Rust core. macOS comes
first; a future Linux frontend remains possible. The main UI will not be a web application.

## Installation requirement

Released versions must support installation and normal operation without administrator
rights on macOS and Windows. Installation, updates, and uninstallation must work within
the current user's account:

- macOS: support installation in `~/Applications`.
- Windows: provide a per-user installer under `%LocalAppData%`.
- Keep bundled Copilot runtime files, settings, caches, sessions, and update staging in
  user-writable locations. Required runtime prerequisites must have a supported path
  that does not require elevation.
- Normal use must not require a privileged service, system-wide PATH changes, or running
  Rivet or its agents as administrator. Separately requested privileged development
  operations require explicit, operation-specific approval.

Release acceptance must verify install, launch, normal use, update, and uninstall on
fresh standard-user accounts on both platforms, without administrator credentials.
Optional installation for all users may require elevation. Installers and this acceptance
verification are future release work; they are not implemented by the current prototype.

## Current stage

Stage 3 adds the native **macOS Copilot workspace**: real session create/resume,
chat streaming, cancellation, model choices, instructions, permission prompts, and
saved session references. Run `cargo run -p rivet -- ui` (or no arguments on macOS).
The earlier mock performance prototype remains available through `ui --smoke`.
Windows and Linux currently provide CLI probes and portable tests.

See [Stage 3 implementation and acceptance gates](docs/stage-3.md) and
[Stage 2 performance evidence](docs/stage-2.md). Real model access remains an
external verification gate; a successful auth-status RPC alone is not sufficient.

Requirements: Rust (pinned by `rust-toolchain.toml`), a native linker, and Git for repository
features. Linux compilation of the SDK requires OpenSSL 3 development files and pkg-config.
The SDK's default feature bundles its compatible Copilot runtime; the first build needs
network access to fetch dependencies and runtime artifacts. Users do not install a Rust SDK.

```sh
cargo run -p rivet -- doctor
cargo run -p rivet -- auth
cargo run -p rivet -- chat "Reply with a short greeting. Do not use tools."
cargo run -p rivet -- chat "Continue our conversation." --resume SESSION_ID
cargo run -p rivet -- chat "Explain the idea of recursion at length." --cancel-after-ms 500
cargo run -p rivet -- resume SESSION_ID
```

Run from the intended project directory (or use `cargo run --manifest-path /path/to/rivet/Cargo.toml ...`).
`auth` checks existing authentication; it does not implement login. Authenticate through
Copilot CLI using its supported login flow. GitHub repository access does not establish
Copilot entitlement. Prompts can consume Copilot usage.

The CLI feasibility probe denies all tool requests; the live Mac UI requires an explicit
allow-once/reject decision for SDK permission requests. This is not OS
sandboxing. Run probes in a disposable workspace without secrets. Session storage belongs
to the SDK/runtime; Rivet only prints session references and does not journal credentials.

## Checks

```sh
cargo fmt --all -- --check
python3 tools/check_dependencies.py
python3 -m unittest discover -s tools -p 'test_*.py'
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

External dependencies are confined to two boundaries: the official SDK, Tokio, and its async-trait integration in
`rivet-copilot`, and macOS-only AppKit/Foundation bindings in `rivet-macos`.
Other crates depend on first-party workspace code.
The lockfile pins transitive dependencies. CI checks macOS, Windows, and headless Linux;
Linux CI does not imply Linux UI support.

Run `cargo run -p rivet -- bench-ui` for portable algorithm measurements. macOS CI also
runs the real native UI smoke test and uploads its screenshot and timing report.

See [Stage 1 verification and remaining gates](docs/stage-1.md).
