# Stage 1: SDK feasibility and foundation

## Decisions verified against the published SDK

- Official package: `github-copilot-sdk`, pinned to `1.0.32-preview.1` (preview API).
- Minimum SDK Rust version: 1.94. Toolchain pinned to 1.99.1 for reproducibility.
- SDK asynchronous process, transport, and session APIs require Tokio. Transitive
  dependencies are not automatically accessible to consumers. The user explicitly
  approved SDK-required dependencies, so Tokio is a direct adapter-only dependency.
- A private current-thread Tokio runtime hosts the probe. It must execute off the UI
  thread when integrated into the future app. It is not the application scheduler.
- Default `bundled-cli` feature acquires and embeds the compatible runtime through the
  SDK build. No custom Copilot transport, protocol, agent engine, or downloader is added.
- SDK types stay internal; application-owned options/events cross the boundary.
- Streaming subscribes before sending; session IDs survive disconnect for later resume.
- SDK abort is cancellation, not pause. Resume means reconnecting a stored session,
  not automatically continuing an interrupted turn.
- DenyAllHandler is installed on creation and resume. This proves safe default wiring,
  not interactive approvals or operating-system isolation.

Sources: [published SDK](https://crates.io/crates/github-copilot-sdk/1.0.32-preview.1),
[official Rust documentation](https://github.com/github/copilot-sdk/tree/main/rust).
Published package source, rather than unreleased main alone, informed implementation.

## Manual live verification (macOS and Windows)

Use a disposable workspace. Install/sign in to Copilot CLI through its supported flow.
Do not paste tokens into prompts, repository files, or issue reports.

1. Run `rivet doctor`. Verify Git is reported or its absence is handled.
2. Run `rivet auth`. Require `authenticated=true` and successful exit.
3. Run `rivet chat "Reply with a short greeting. Do not use tools."`.
   Record session ID; require streamed text and `completed`, with successful exit.
4. Run a second process: `rivet chat "What was my previous request?" --resume ID`.
   Verify contextual continuity. `rivet resume ID` tests reconnect without sending.
5. Run `rivet chat "Explain recursion at length." --cancel-after-ms 500`.
   Require cancellation acknowledgement (a turn finishing first is not a cancellation pass).
   Resume the session to check usability after abort.
6. Ask to create a harmless file in this disposable directory. Confirm denial and
   independently verify the file was not created. Repeat after session resume.
   A model declining to request a tool does not count as a live permission-hook test.
7. Test invalid session ID, missing authentication, short timeout, and unavailable runtime.
   Require nonzero exit with an error and no crash; inspect for leaked child processes.

Never treat mock/unit tests as proof of successful model communication. CI intentionally
runs no paid prompts and receives no Copilot credentials.

## Timing boundaries

The probe prints SDK startup and send-to-first-delta microseconds. These include runtime,
network, and model time; they are not UI latency measurements. Stage 2 adds native UI
instrumentation, p50/p95/p99 benchmarks, and reference hardware.

## Remaining gates

- Authenticated create, streaming, abort, resume, and live permission checks require
  Copilot access and must be recorded per OS before declaring SDK feasibility proven.
- macOS and Windows runtime behavior requires native execution; headless Linux cannot
  establish native platform acceptance.
- CI verifies the dependency graph and build, not real authentication or tool isolation.
- Stage 2 will implement the macOS shell. No platform crates or empty subsystem crates
  are created in this stage.
- Runtime redistribution/license review, installers, credential UX, and OS containment
  remain release work; bundling for the probe does not establish release readiness.

## Verification recorded in the development environment (2026-11-11)

Host: Linux x86_64, Rust 1.99.1; only the Linux Rust target is installed.
This is SDK/headless verification, not Linux UI implementation.

- Workspace compilation, four Rust tests, three Python policy tests, formatting,
  dependency enforcement, and Clippy with warnings denied: passed.
- `doctor`, help, invalid duration rejection, and missing Git handling: passed.
- Actual bundled runtime startup and authentication-status RPC: passed. Status
  reported authenticated; cold extraction/startup was about 4.25 seconds. This is
  not the future application shell startup metric.
- Actual session creation and separate-process resume: passed.
- Abort while a request was pending: SDK acknowledged; process exited successfully.
  Cancellation during active streamed inference remains unverified.
- Actual model request: failed with authorization HTTP 401 (credentials expired or
  invalid), despite authentication status reporting true. The probe exited nonzero
  without crashing. No streamed model output or contextual continuation is claimed.
- Live permission-triggering tool execution: blocked by model authorization. The
  real built-in denial handler is unit-tested, and is configured on create/resume.
- macOS/Windows compilation is delegated to the committed CI matrix; inspect the
  workflow results before considering those platform build gates satisfied.

Stage 1's executable foundation is implemented. Its complete live feasibility gate
remains open until valid Copilot model access and native-platform verification are
available. Do not advance to Stage 2 based on an assumption that those gates passed.
