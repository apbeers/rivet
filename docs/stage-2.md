# Stage 2: native macOS performance prototype

## Scope

A real AppKit window, implemented in Rust through established native bindings.
No browser-rendered application UI, no real agent sessions, and no filesystem or
process execution from the prototype. Every conversation and setting is labeled mock.
The Stage 1 CLI remains available. The user authorized this independent UI work
while Stage 1's model-authorization/live permission verification gates remain open.

macOS is the first UI implementation. Windows still builds the CLI and portable
state code; its native frontend follows later. Linux has no UI. Platform-neutral
state has no Objective-C types, async runtime, or unsafe code.

## Run

On macOS with Xcode command-line tools and the pinned Rust toolchain:

```sh
cargo run -p rivet -- ui
# On macOS, no arguments also opens the native UI.
cargo run -p rivet
```

Use the sidebar to switch among 20 persistent mock sessions. Scroll a conversation
with the native scroll view/trackpad. Draft text in the native input field, open the
Agent settings tab, and choose a mock model. Drafts, scroll offsets, settings-tab
selection, and model selection survive navigation. Live activity jumps to the
mock streaming row. Closing the window or Command-Q exits the prototype.

## Implementation boundaries

- `rivet-prototype`: session state, immutable fixture records, fixed-height cached
  row layout, viewport range calculation, bounded telemetry, one mock worker.
- `rivet-macos`: AppKit lifecycle, native controls, text drawing, clipping, target/
  action routing, native scrolling, cached row labels, and the custom conversation view.
- Native bindings (`objc2`, `objc2-foundation`, `objc2-app-kit`) are macOS-only direct
  dependencies of `rivet-macos`. This implements the user's approved native-binding
  exception; dependency policy checks enforce its package and target boundaries.
- All 20 sessions share 100,000 immutable synthetic records to avoid duplicating
  identical fixture strings. Each session owns its draft, model, scroll offset,
  tab state, and bounded live text. There is one extra live-activity row.
- Conversations have fixed 72-point rows; body text is clipped to the row. This
  proves viewport scaling, not variable-height Markdown/rich-text performance.
- The native conversation view paints only the visible/damaged rows. AppKit owns
  backing, clipping, text shaping, scrollbar behavior, and momentum scrolling.
- One bounded channel (64 events), nonblocking producer, at most 32 events per UI
  tick. The worker updates one session every 10ms, independently of navigation.
  Saturation may drop MOCK updates. This is not a production streaming policy.
- Live updates invalidate only the last row, when visible. They do not append
  history rows or move the scroll anchor. Incremental arbitrary insertions and
  variable-height anchoring remain later work.
- Native views are reused; switching sessions does not recreate session state.

## Verification and performance evidence

Portable checks:

```sh
cargo test --workspace --locked
python3 tools/check_dependencies.py
python3 -m unittest discover -s tools -p 'test_*.py'
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run --locked -p rivet -- bench-ui
```

`bench-ui` measures only state lookup/switch and visible-range algorithms, across
20 sessions and 100,000 records. It excludes native layout, drawing, presentation,
input delivery, and model/network work. Do not use its nanosecond results as UI latency.

Native checks (requires an actual macOS graphical session):

```sh
cargo build --locked -p rivet
python3 tools/run_macos_smoke.py
```

The smoke process opens the window, exercises actual native button actions, checks
Unicode draft/scroll restoration and cached settings, then cycles through 20 sessions
and scrolls across the synthetic history while mock updates continue, followed by
50 cached settings open/close cycles. It requires
real drawing callbacks, checks viewport-bounded row counts, saves a native PNG,
and exits automatically. The Python wrapper enforces a 90-second process timeout,
requires sufficient native samples, and records host details, process CPU time,
and peak RSS. macOS CI uploads the log and image as `macos-ui-smoke`.

Measured distributions (bounded rolling window of 4096 samples):

- Session-switch and settings CPU callback time.
- Conversation draw callback CPU time.
- Session selection to the next conversation draw callback (not screen presentation).
- Arrival of a visible mock activity update to its draw callback.
- Frontend entry to the first conversation draw callback (excludes OS process launch).

CPU regression budgets are p99 <16ms for switching and <8ms for drawing, on the CI
runner (also <16ms for cached settings callbacks). Passing these does not prove full interaction latency or 60/120Hz display
performance. Callback intervals are not presented frame times. Reference-hardware
interactive measurements, display synchronization, and long-session profiling remain
acceptance work. No unsupported performance target is claimed.

## Manual macOS acceptance

Check trackpad momentum, scrollbar dragging, window resizing, Retina scale, dark/light
appearance, Unicode/IME editing, keyboard focus and native text shortcuts. Scroll into
the middle, switch sessions and return, and confirm the same content remains visible.
Observe updates in Live activity while switching and scrolling. Inspect Instruments
for frame pacing, CPU, and allocations on identified reference hardware at 60/120Hz.

Native controls retain platform accessibility behavior. The custom conversation
surface is not yet an accessible per-message document or selectable text editor.
This remains an explicit limitation, along with no cross-process UI-state persistence,
no actual Copilot UI integration (Stage 3), and no Windows frontend yet.
