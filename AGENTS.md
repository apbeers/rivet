# Rivet engineering rules

Read README.md and docs/stage-1.md before changes.

- Build only the stage requested. Stage 2 is a mock native macOS UI, not real SDK integration.
- macOS is the first UI target; Windows follows. Keep a future Linux frontend possible.
- Releases must support per-user install, normal operation, update, and uninstall without
  administrator rights; see README.md. Keep required runtimes and application data user-writable.
- Use native platform UI and a shared Rust core. Custom Rust surfaces may serve demanding views.
- External dependencies: SDK and Tokio only in rivet-copilot; objc2, objc2-foundation,
  and objc2-app-kit only in rivet-macos under macOS conditional dependencies.
- Read docs/stage-2.md for prototype limitations and verification gates.
- Never expose SDK or Tokio types through the adapter's public boundary.
- No credentials in logs, source, snapshots, or Git. Tool permissions fail closed.
- Permission hooks are not an OS sandbox. Do not claim sandboxing.
- Do not claim authenticated or cross-platform checks passed without execution evidence.
- Run formatting, dependency policy and its tests, workspace tests, and Clippy before commits.
