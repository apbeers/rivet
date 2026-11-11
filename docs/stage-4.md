# Stage 4 — Mac browser and workspace documents

The native workspace now has Agents, Browser, and Documents views. Browser tabs use
WKWebView with address navigation, back/forward/reload and localhost URL support.
Switching a tab reuses its native web view. A five-instance LRU budget evicts old views;
this bounds instance count, not measured WebContent process RSS. Byte-based browser
memory pressure handling and warm-tab presentation benchmarks remain acceptance work.

Only HTTP/HTTPS addresses and about:blank can be entered through the address bar.
No privileged JavaScript bridge is installed. The browser never renders Rivet's shell.
WebKit owns page loading and its own cookie/credential storage. Network navigation
is asynchronous; native view creation is lazy and necessarily on the main thread.

Workspace browsing uses a cancellable standard-library filesystem worker with bounded
queues, 16 MiB per-file reads, a 32 MiB/20-entry immutable LRU cache, UTF-8 validation,
and canonical workspace containment including symlink escapes. The native document
cache independently bounds text to 32 MiB/20 entries and preserves native scroll and
selection state. Read-only document views are separate from agent conversations.
The directory popup displays at most 200 entries to bound control construction;
Open path accepts other workspace-relative files. No indexing claim is made.

Portable tests verify cache reuse/invalidation, binary rejection and path containment.
The native controls smoke now creates five browser instances, verifies reuse by native
identity, rejects a JavaScript address, and displays a cached Unicode document fixture.
This does not validate remote GitHub authentication, page developer tools, browser
navigation state under eviction, or the future Windows WebView2 backend. Windows
native UI and byte-budget browser profiling remain explicitly open.
