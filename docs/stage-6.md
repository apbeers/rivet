# Stage 6: development services

The macOS Tools pane launches installed executables with an exact argument array,
bounded output, cancellation, and timeouts on two background workers. The first click
previews a command; the second confirms that same command and workspace. Git status
and Git HEAD diff are available directly. Missing executables produce errors.

The first-party stdio JSON-RPC/LSP transport supports initialization, diagnostics,
hover, definition navigation, references, completion results, and semantic colors.
Enter a language server executable and JSON argument array, then Start LSP / Confirm.
Open a document before requesting language features. Definition results remain subject
to workspace containment when opened. Server-initiated edits are rejected.

Native macOS PTY sessions use the user's installed shell. The console routes keyboard,
arrows, paste, and control input to the PTY; password echo belongs to the terminal driver.
Four sessions are supported. The bounded first-party renderer handles common cursor,
erase, scrolling, and UTF-8 sequences. It is not complete xterm emulation: wide glyphs,
complex graphemes, mouse protocols, full-screen programs, terminal colors, and IME
composition require further implementation and native testing.

Limits: process cancellation terminates the immediate process; descendant containment
is not guaranteed. PTY process-group cleanup is best effort. No OS sandbox is claimed.
LSP uses full document reopen on explicit requests after edits (256 KiB budget), not
incremental synchronization; completion items are displayed but not applied. Process
output is presented on completion, not streamed. Automatic repository-wide search,
debugging, file watching, and dedicated pull-request flows remain open; installed `gh`
and development commands can be launched through Tools with explicit confirmation.

Portable tests exercise real processes, timeout/error handling, and a real stdio LSP
fixture. The macOS live smoke additionally exercises the native tools pane, actual PTY,
and process/LSP transport. The fixture is explicitly test-only and proves no model access.
