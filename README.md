# Rivet

A Rust-native workspace for AI agents, built for responsiveness and developer control.

Rivet will use native macOS and Windows interfaces over a shared Rust core. macOS was the first frontend; Windows now has a native Win32 backend.
A future Linux frontend remains possible. The main UI will not be a web application.

## Download and run

[**Download Rivet Preview**](https://github.com/apbeers/rivet/releases/tag/preview)
— ready-to-run packages from passing `main` builds. No Rust, Cargo, or Copilot Rust
SDK installation is needed; the compatible Copilot runtime is bundled.

| Your computer | Download |
| --- | --- |
| Windows 10 1809+ / Windows 11, x64 | [**Windows ZIP**](https://github.com/apbeers/rivet/releases/download/preview/Rivet-Windows-x64.zip) |
| macOS 13+, Apple Silicon (M1 or newer) | [**Mac ZIP**](https://github.com/apbeers/rivet/releases/download/preview/Rivet-macOS-arm64.zip) |

### Windows

1. Download the Windows ZIP and choose **Extract All**. Keep all extracted files together.
2. Double-click **Rivet.exe** in the extracted folder. No installer or administrator
   rights are needed. You can move that folder anywhere in your account.

Embedded browsing requires Microsoft's [WebView2 Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/);
most Windows installations already have it. If missing, use Microsoft's installer
without elevation for a per-user install.

### Mac

1. Download the Mac ZIP and unzip it.
2. Double-click **Rivet.app**. No installer or administrator rights are needed.
3. Optionally copy it into your home Applications folder (`~/Applications`). In Finder,
   use **Go → Go to Folder → `~/Applications`** to find that folder or create it first.

This package is for Apple Silicon.
The preview is ad-hoc signed and not notarized. If macOS blocks opening it, follow
[Apple's per-app opening instructions](https://support.apple.com/en-us/102445) for an
app you trust. Windows previews are unsigned and may show an unknown-publisher prompt.

### Connect your first agent

1. Authenticate once using [GitHub's Copilot CLI login instructions](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli):
   run `copilot login` if Copilot CLI is installed. If needed, use its
   [official installation guide](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli).
2. Open Rivet, set the workspace directory to your project, and click **Connect**.
3. Enter a development task and click **Send**. Review tool requests before allowing them.

You need a GitHub account with Copilot access. Agent prompts may consume your Copilot
usage. Git is needed for Git/worktree features; language servers are optional installed
tools. The workspace UI opens without those tools, and missing tools report errors.
An in-app login flow is not implemented yet.

### Update or uninstall

Quit Rivet, download the current ZIP from the same links, and replace your extracted
Windows folder or Mac app with the new copy. Delete the folder/app to remove Rivet.
Settings/session references remain in your account's Rivet data directory.

[SHA-256 checksums](https://github.com/apbeers/rivet/releases/download/preview/SHA256SUMS.txt)
and [build details](https://github.com/apbeers/rivet/releases/download/preview/BUILD-INFO.txt)
identify each download. Preview downloads use development/debug builds. Native CI checks
do not establish real-model access, fresh standard-user acceptance, or reference-hardware
performance; these remain open gates. See [Windows limits](docs/windows.md) and
[hardening/release gates](docs/stage-8.md).

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
verification remain release gates. A development macOS bundle and per-user install/update
script are available; see [Stage 8](docs/stage-8.md).

## Current stage

Stages 3–5 add the native **macOS Copilot workspace**: real session create/resume,
chat streaming, cancellation, model choices, instructions, permission prompts, and
saved session references. Run `cargo run -p rivet -- ui` (or no arguments on macOS).
Browser and Documents views add retained WKWebView tabs and workspace file browsing
with bounded background loading/caching. See [Stage 4](docs/stage-4.md).
Documents now include first-party piece-table editing, undo/redo/search, checked atomic
saves on Mac, and inline/split review of unsaved edits. See [Stage 5](docs/stage-5.md)
for limits and remaining acceptance work.
The earlier mock performance prototype remains available through `ui --smoke`.
The Tools pane adds background commands, Git inspection, LSP requests, and constrained
native PTY sessions ([Stage 6](docs/stage-6.md)). Tasks adds durable dependency scheduling,
isolated worktrees, SDK sessions, validation and reviewed integration ([Stage 7](docs/stage-7.md)).
Windows now includes native sessions, documents/review, tools, ConPTY terminals, task
supervision and WebView2 browser tabs. Run `rivet.exe` or `rivet.exe ui`; see
[Windows implementation, packaging and limits](docs/windows.md). These remain development
features with explicit acceptance gaps. Linux provides CLI probes and portable tests.

See [Stage 3 implementation and acceptance gates](docs/stage-3.md) and
[Stage 2 performance evidence](docs/stage-2.md). Real model access remains an
external verification gate; a successful auth-status RPC alone is not sufficient.

## Build from source

Build requirements: Rust (pinned by `rust-toolchain.toml`), a native linker, and Git for repository
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
`rivet-copilot`, and macOS-only AppKit/Foundation bindings in `rivet-macos`. Windows uses handwritten OS
bindings and first-party Rust crates.
Other crates depend on first-party workspace code.
The lockfile pins transitive dependencies. CI checks macOS, Windows, and headless Linux;
Linux CI does not imply Linux UI support.

After all checks and extracted-app launch tests pass, CI prepares the two native ZIPs
with architecture checks, SHA-256 checksums and source/build references. Only `main`
publishes the managed `preview` prerelease; feature branches validate packaging without
publishing. The preview tag moves with published builds, keeping the download links
above stable. Updates to the application itself are manual.

Run `cargo run -p rivet -- bench-ui` for portable algorithm measurements. macOS CI also
runs the real native UI smoke test and uploads its screenshot and timing report.

See [Stage 1 verification and remaining gates](docs/stage-1.md).
