# Rivet engineering rules

Read README.md and docs/stage-1.md before changes.

- The user authorized autonomous work through stages 3–8, committing and pushing each stage.
  Keep the Stage 2 mock benchmark separate from production features; record unmet gates.
- macOS is the first UI target; Windows follows. Keep a future Linux frontend possible.
- Releases must support per-user install, normal operation, update, and uninstall without
  administrator rights; see README.md. Keep required runtimes and application data user-writable.
- Use native platform UI and a shared Rust core. Custom Rust surfaces may serve demanding views.
- External dependencies: SDK, Tokio, and async-trait only in rivet-copilot; objc2, objc2-foundation,
  objc2-app-kit, and objc2-web-kit only in rivet-macos under macOS conditional dependencies.
- Read docs/stage-2.md for prototype limitations and verification gates.
- Never expose SDK or Tokio types through the adapter's public boundary.
- No credentials in logs, source, snapshots, or Git. Tool permissions fail closed.
- Permission hooks are not an OS sandbox. Do not claim sandboxing.
- Do not claim authenticated or cross-platform checks passed without execution evidence.
- Run formatting, dependency policy and its tests, workspace tests, and Clippy before commits.
