# Native Windows development build

Windows x64 now has its own Win32 frontend over the same Rust application model,
SDK adapter, piece table, task coordinator, process supervisor, LSP client and journal.
No new external Cargo dependency is added. Handwritten bindings stay in the Windows
crate, compiled only on Windows. macOS and future Linux UI backends remain independent.

Run `rivet.exe` or `rivet.exe ui` to open the workspace. Agent sessions retain their
native conversation controls, drafts, selection and scroll state while switching.
SDK create/resume, streaming, cancellation, models, instructions, and explicit tool
approvals use the existing adapter. Interactive turns are serial; Tasks provides
parallel SDK sessions in isolated worktrees and validates before reviewed integration.

Documents cache native RichEdit views while Rust owns edit operations, UTF-8 storage,
UTF-16 conversion, undo/redo, source search and diff generation. Ordinary typing, paste,
cut, delete and keyboard undo update the piece table incrementally. CRLF files use a
normalized editor buffer and retain CRLF when saved. Mixed/legacy CR line endings are
read-only in this version. An uncommon native edit path (e.g. some IME operations) uses
a bounded 256 KiB fallback; larger unsupported edits are reverted with an error.
Saving stages on a worker, checks for external changes, flushes content and uses
Windows `ReplaceFileW` to preserve the original file's DACL/streams. This is not a
filesystem compare-and-swap or proof of power-loss durability for all filesystems.

Tools supports exact-command confirmation, bounded output, cancellation, Git status
and HEAD diffs, and installed stdio language servers. Hover, definition navigation,
references/completion results, diagnostics and semantic colors share the first-party
LSP transport. Full-document LSP synchronization is limited to 256 KiB; completion
insertion and incremental synchronization remain open as on macOS.

ConPTY runs shells under a kill-on-close Job Object. Pipe input and output have separate
workers; output continues draining during console shutdown. Four retained sessions use
the constrained first-party UTF-8/VT grid. Full terminal emulation, wide glyph widths,
complex graphemes, terminal IME behavior and full-screen applications remain open.
Job supervision does not provide filesystem or credential sandboxing.

Browser tabs use WebView2 asynchronously on the UI's COM STA. Five retained controllers
are cached; warm switching does not recreate them. Only HTTP(S) and about:blank can be
entered into the host address bar, and no privileged JavaScript bridge is installed.
The official native loader is pinned by package version and SHA-256, with its license
included. Microsoft Evergreen Runtime is required; missing loader/runtime produces an
error while other tools remain available. `rivet.exe webview-version` probes availability.
Browser count is bounded; an RSS budget, suspending background pages, comprehensive
popup handling, and browser crash recovery are still release work.

## Per-user packaging

`tools/package_windows.ps1` creates `dist/Rivet-Windows-x64-development.zip`, embeds
an `asInvoker`/PerMonitorV2 manifest, and includes the loader and install/uninstall scripts.
MSVC builds statically link the C runtime to avoid an elevated VC++ installer prerequisite.
Run `Install-Rivet.ps1` from the extracted package to install/update under
`%LocalAppData%/Programs/Rivet`; no service, registry registration or PATH change is made.
Quit Rivet before updating. `Uninstall-Rivet.ps1` removes only known application files;
settings and SDK references remain under `%LocalAppData%/Rivet` for explicit user removal.
The product installer never automatically installs development tools or browser runtimes.
Microsoft's Evergreen installer supports per-user installation when run without elevation.

This is an unsigned development build. Authenticode/SmartScreen, fresh standard-user
install/use/update/uninstall acceptance, accessibility, keyboard/IME coverage, real model
access, and reference-hardware latency/memory measurements remain release gates. The
headless/Linux environment can cross-check Rust types but cannot establish Windows UI
behavior. Native CI executes a real window and checks actual source saving, subprocesses,
LSP, ConPTY and local HTTP WebView2 navigation. Only the agent transport is a labeled fixture;
no fixture is used by normal production launch. Inspect CI results before claiming a pass.
