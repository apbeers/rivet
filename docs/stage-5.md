# Stage 5 — editor and unsaved source review

Documents now use a native editable text view backed by a first-party Rust piece table.
The table retains immutable UTF-8 spans, maintains byte/UTF-16 line starts, validates
surrogate and UTF-8 boundaries, supports bounded undo/redo history and streaming search
across pieces. Native selection, clipboard, scrolling and input methods remain AppKit
responsibilities. Load-time line indexing occurs on the filesystem worker.

Initial lexical coloring handles comments, quoted strings, numbers and common keywords
within an 8 KiB/512-token budget. This is intentionally lightweight highlighting, not
language analysis. The editor rejects changes beyond the 16 MiB document budget.

Accept + Save writes a captured piece snapshot on the filesystem worker. The worker
checks the disk against the loaded base, writes/syncs a sibling temporary file, preserves
permissions, checks again for external changes, atomically renames on macOS, and syncs
the parent directory. Detected external edits abort saving. This is not an OS sandbox
or a filesystem compare-and-swap guarantee against hostile concurrent directory changes.
Windows atomic replacement is explicitly unsupported until its native backend exists.

Unsaved diff runs on the worker and offers inline and side-by-side colored excerpts.
It compares the editor base with the current buffer, not Git HEAD. The conservative
replacement hunk is bounded and is a review format, not an applyable patch. Rejecting
unsaved edits requires a second confirmation bound to the specific file and revision;
it leaves the disk untouched. Dirty documents cannot silently disappear on quit or
window close. Native view eviction preserves dirty buffers.

Portable tests cover Unicode/surrogate edits, random edit sequences against a reference
string, undo/redo, cross-piece search, line counts, diff replacement/newline cases,
atomic save conflict rejection and permissions. The native controls test now edits a
fixture through the text-view permission/delegate path, tests undo/redo/search, requests
an asynchronous diff and explicitly rejects that fixture's unsaved changes.

Remaining acceptance work: actual IME/keyboard interaction, editor line-number gutter,
large-file editing benchmarks and allocations, whole-document/incremental semantic
coloring, conflict reconciliation UI, Git-based review and automatic file-change alerts.
These are not represented as complete. Stage 6 development services supply the Git,
watching, language-server and execution integration next.
