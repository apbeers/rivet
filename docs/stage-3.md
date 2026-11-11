# Stage 3 — live Copilot workspace

Implemented: a native Mac workspace, persistent SDK sessions behind a bounded worker
interface, real send/stream/cancel APIs, per-session drafts, new-session instructions
and SDK model choices, saved session references and resume, tool activity, and explicit
allow-once/reject controls. The UI starts without launching the SDK. Session work,
permission waits and durable writes run off the UI thread. Native UI types and SDK
runtime types remain on their respective sides of a first-party backend interface.

The application stores only SDK session references and permission decision audit
records in a private, versioned/checksummed journal under the user's data directory.
Torn records are detected and truncated; checksum corruption is reported and preserved.
The SDK owns full conversation history; resume projects recent SDK messages into a
bounded resident transcript. App model/instruction controls apply when connecting;
these fields are not separately persisted by Rivet across application restarts.

Tool requests expire after 120 seconds and default to rejection. Approving one request
requires its decision to reach durable storage first. A full queue, unavailable UI,
missing decision, or storage failure cannot authorize an operation. These hooks are
not an OS sandbox; running a privileged command or unrestricted process is not isolated.
Closing the application requests SDK shutdown before allowing native termination.

Verification: portable journal/state tests, real permission-handler tests, macOS
cross-check, and CI native controls test using an explicitly named test backend.
The Stage 2 performance smoke remains separate (`ui --smoke`). `ui --live-smoke` tests
the production controls against fixtures; it does not establish real model access.

A real `cargo run -p rivet-copilot --example host_probe` run created a session but
failed at the model turn with a session error in this environment. Successful live
model streaming and live tool permission execution therefore remain acceptance gates.
The previous 401 authorization finding has not been assumed fixed. Windows native UI
and per-message accessibility remain open. No mock text is used in the normal UI.
