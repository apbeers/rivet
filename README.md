# Rivet

A Rust-native workspace for AI agents, built for responsiveness and developer control.

Rivet will use native macOS and Windows interfaces over a shared Rust core. macOS comes
first; a future Linux frontend remains possible. The main UI will not be a web application.

## Current stage

Stage 1 implements a small command-line Copilot SDK feasibility probe and repository checks.
There is **no desktop UI, editor, orchestration, or application persistence yet**.

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

All requested tool permissions are denied in this feasibility slice. This is not OS
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

Only `rivet-copilot` directly depends on external crates: the official SDK and Tokio,
which the SDK requires for execution. Other crates depend on first-party workspace code.
The lockfile pins transitive dependencies. CI checks macOS, Windows, and headless Linux;
Linux CI does not imply Linux UI support.

See [Stage 1 verification and remaining gates](docs/stage-1.md).
