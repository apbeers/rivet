Native Windows and macOS development previews. Rust and the Copilot Rust SDK are
not required to run these downloads; the compatible Copilot runtime is bundled.

| Platform | Download |
| --- | --- |
| Windows 10 1809+ / Windows 11, x64 | [Rivet-Windows-x64.zip](https://github.com/apbeers/rivet/releases/download/preview/Rivet-Windows-x64.zip) |
| macOS 13+, Apple Silicon (M1 or newer) | [Rivet-macOS-arm64.zip](https://github.com/apbeers/rivet/releases/download/preview/Rivet-macOS-arm64.zip) |

**Windows:** Extract the entire ZIP and double-click `Rivet.exe` in the extracted folder.
Keep the other files beside it. Embedded browsing requires Microsoft's
[WebView2 Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

**Mac:** Unzip and double-click `Rivet.app`. You can also copy it to your own Applications
folder. This download is for Apple Silicon.

No installer, Rust toolchain or administrator rights are needed. Quit Rivet before
updating, download the new ZIP, and replace the old folder/app. Delete it to uninstall.

Before using agents, [sign in to Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli)
with `copilot login`. Choose a project folder in Rivet, connect a session, and send
a task. Copilot access is required; prompts may consume your usage. Git is needed
for repository operations.

These are debug development builds. Windows is unsigned; the Mac app is ad-hoc
signed and not notarized. See [installation and first-run help](https://github.com/apbeers/rivet#download-and-run),
including macOS opening restrictions. Real model access, fresh standard-user
acceptance and reference-hardware performance remain open verification gates.

[SHA-256 checksums](https://github.com/apbeers/rivet/releases/download/preview/SHA256SUMS.txt)
and [build provenance](https://github.com/apbeers/rivet/releases/download/preview/BUILD-INFO.txt)
identify the exact downloadable build. This is a rolling preview, not a stable release.
