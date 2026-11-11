# Rivet engineering rules

Read README.md and docs/stage-1.md before changes.

- Build only the stage requested. Stage 1 has no desktop UI.
- macOS is the first UI target; Windows follows. Keep a future Linux frontend possible.
- Use native platform UI and a shared Rust core. Custom Rust surfaces may serve demanding views.
- External direct dependencies are confined to rivet-copilot: the official Copilot SDK and Tokio.
  Native bindings are an approved architectural direction, but are not introduced in Stage 1.
- Never expose SDK or Tokio types through the adapter's public boundary.
- No credentials in logs, source, snapshots, or Git. Tool permissions fail closed.
- Permission hooks are not an OS sandbox. Do not claim sandboxing.
- Do not claim authenticated or cross-platform checks passed without execution evidence.
- Run formatting, dependency policy and its tests, workspace tests, and Clippy before commits.
