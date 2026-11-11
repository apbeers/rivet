# Stage 8: hardening and release gates

Implemented in the consolidated development build:

- Journal read/write access supports torn-tail truncation on Windows. An exclusive
  sidecar lock survives atomic checkpoint replacement. Versioned/checksummed snapshots
  compact task state every 128 persisted transitions. Uncommitted temporary snapshots
  never supersede the journal. Unix file permissions are private for journal records.
- Services use bounded queues and resource limits. UI shutdown requests SDK, process,
  language-server, task, and PTY shutdown and polls acknowledgements without joining on
  the AppKit thread. Process errors clean up the immediate child.
- Background SDK model-list/session-ready events preserve drafts. Cached document
  selection uses stable popup keys rather than a changing LRU index.
- `tools/package_macos.py` produces an ad-hoc-signed development `.app` and zip from a
  built binary; `tools/install_macos.sh /path/to/Rivet.app` stages and verifies a per-user
  install/update under `~/Applications`, with rollback if the final move fails. No sudo,
  privileged helper, or system PATH changes. Quit the app before updating. Uninstall by
  moving `~/Applications/Rivet.app` to Trash; application data remains separately under
  `~/Library/Application Support/Rivet` unless explicitly removed by the user.
- CI uploads timeout logs and native process samples rather than losing diagnostics.

These are implementation slices, not a declaration that the eight-phase specification is
complete. Remaining release gates include:

1. Real Copilot model streaming/permission/multi-agent tests with working entitlement.
2. Native macOS execution of the consolidated tests and reference-hardware latency,
   memory, startup, and long-session benchmarks; cold shell performance remains unproven.
3. Windows UI, browser, ConPTY, atomic source saves, and per-user packaging. Linux UI is
   deferred. Windows/Linux CLI and shared-core CI do not imply native desktop support.
4. Full accessibility/VoiceOver and IME testing, large-document budgets covering all
   editor allocations, unsaved-edit crash restoration, automatic file watching, advanced
   diff/conflict editing, full terminal emulation, incremental LSP/completion insertion.
5. Production process containment; credential-manager/login UX; runtime redistribution
   review; Developer ID signing, notarization, updater trust, and standard-user install,
   launch, update, and uninstall acceptance. Ad-hoc signing does not establish Gatekeeper
   distribution trust. No signing credentials are available in this environment.
6. Browser RSS budgeting/suspension and actual presented-frame metrics on reference
   Windows and Mac hardware. View-count budgets and callback timings are not substitutes.

Run local checks before hosted pipelines. Keep production functionality distinct from
fixtures, and never describe outstanding gates as passed.

## Shared editor measurements (Linux x86_64 development container)

`cargo run --release -p rivet-core --example bench_editor` records buffer CPU timings,
not native layout, input-to-display latency, memory budgets, or file I/O. A 10 MB-scale
buffer with 403,299 lines took about 13 ms to construct. Over 100 samples, inserting
at its first line measured p50 776 us, p95 942 us, p99 2,890 us. Appending to a 10 MiB
single-line buffer measured p50 1 us, p95 2 us, p99 3 us after caching UTF-16 lengths
per piece (previous p50 15,594 us). These are one container run, not reference-hardware
acceptance. CI records the same benchmark on its three OS runners. Native live workspace
shutdown also reports bounded p50/p95/p99 CPU samples for session selection, event
application, and polling; none of these claims presented-frame latency.
