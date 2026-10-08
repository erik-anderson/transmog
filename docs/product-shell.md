# Transmog product shell

The Windows desktop shell is a thin Tauri adapter over `transmog-app`. The same
application facade is suitable for a future CLI: it owns proxy lifecycle,
bounded session queries, safe inspectors, breakpoints, replay, capture, import,
export, product state, and privacy-safe diagnostics without depending on Tauri
or WebUI.

## Operating model

- **Set up HTTPS interception** creates a durable PEM CA and matching private
  key when needed, then explicitly asks the user to approve current-user trust.
  CA generation is create-new and never overwrites either file. Start verifies
  the files, recorded SHA-256 identity, and current trust state before binding.
- Desktop Start always journals the exact current-user Windows proxy registry
  values, starts an automatic bounded native capture, binds the listener, and
  then applies the loopback proxy. Stop and normal/OS-requested exit restore the
  exact prior values. The durable journal is recovered on next launch after a
  hard termination.
- Certificate trust installation/removal remains an explicit exact-SHA-256
  operation and can display an OS consent dialog. The setup action warns before
  triggering that dialog; unattended builds and tests never install trust.
- The traffic list queries bounded pages (100 by default, at most 200) over
  filtered and sorted retained metadata, and watches live automatically. It
  follows the latest row until the user scrolls away or selects a request;
  capture continues while that view is pinned.
- Traffic rows show the best-effort local caller process name and PID captured
  at connection accept time. Non-loopback peers show as remote and unresolved
  loopback callers remain explicitly unknown.
- Completed retained client responses can seed auto-responses directly from
  the Traffic inspector or multiselection review. The Automation
  workspace exposes a dense priority list and editable properties, with bulk
  actions, reversible deletion, duplicate warnings and a network-free matcher
  tester. The first enabled match wins; exact addresses, guided URL patterns
  and bounded regex are supported. A shared Traffic/Automation pause switch
  controls rule hooks without modifying rule states. Saved responses remain
  editable independently of the original Traffic entry. Winning traffic is
  marked `AUTO` and names the rule and immutable asset that served it. See
  [automation.md](automation.md) for matching and interaction details.
- The primary desktop canvas is a fixed-viewport traffic workspace: an
  internally scrolling request list remains visible above a persistent split
  request/response inspector. Tool views use the left rail and scroll only
  inside the application viewport; the document root never scrolls.
- The `system` theme follows the host light/dark preference live. Explicit
  light and dark choices override it and keep Monaco aligned with the shell.
- Response-body retention is enabled by default with a bounded one-GiB circular
  store and can be disabled. Text, binary, missing,
  truncated, redacted, and lossy evidence are distinct inspector states.
- Interactive breakpoints use one same-build controller. Closing the window,
  disabling breakpoints, timing out, or losing the controller fails unresolved
  decisions closed.
- Composer replay requires explicit acknowledgement for non-idempotent methods
  and credential-bearing fields and uses the canonical Rust HTTP/TLS stack.
- Native `.tmcap` files are the streaming source of truth. The desktop captures
  automatically while its proxy is running and can export a create-new, sealed
  TMCap snapshot without stopping the live source. JSONL export streams records
  sequentially. SAZ must be finalized and cannot preserve every native boundary
  or hook record, so each result includes a fidelity disclosure.
- Raster previews are decoded and normalized to PNG in an AppContainer helper,
  then displayed only through an opaque `<img>` URL. SVG stays SVG for fidelity
  but is also loaded only as an image from a no-store, `nosniff`, sandboxed CSP
  response that blocks scripts and external resources.
- Windows launches the script sandbox bootstrap, preview bootstrap, and
  PowerShell host helpers without creating visible console windows.
- Product state is schema-versioned, bounded, and committed as atomic
  generations. Corrupt newest state falls back to a prior valid generation or
  safe defaults. Persistence failure never blocks proxy shutdown.
- Structured diagnostics are bounded and redacted before memory or disk. A
  support bundle excludes traffic bodies, header values, credentials, keys,
  and paths by default, and reports the native WebView runtime version.

## File safety

Captures, generated CAs, JSONL, and SAZ use create-new semantics. Existing
destinations are not replaced. Imports are subject to explicit file, record,
and record-size bounds; interrupted native files recover only their checksummed
valid prefix.

Build and Windows prerequisites are documented in [building.md](building.md).
Product-state and support behavior is documented in
[product-state-and-support.md](product-state-and-support.md). Current gaps and
deliberately deferred platform work are maintained only in the
[roadmap](roadmap.md). Windows release qualification and eventual Linux/macOS
qualification remain separate platform gates.
