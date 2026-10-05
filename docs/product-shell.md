# Transmog product shell through Phase 8

The Windows desktop shell is a thin Tauri adapter over `transmog-app`. The same
application facade is suitable for a future CLI: it owns proxy lifecycle,
bounded session queries, safe inspectors, breakpoints, replay, capture, import,
export, product state, and privacy-safe diagnostics without depending on Tauri
or WebUI.

## Operating model

- Starting the proxy requires an existing PEM CA certificate and matching
  private key. CA generation is create-new and never overwrites either file.
- Current-user Windows proxy configuration is optional. When enabled,
  `transmog-host-windows` journals the exact prior registry values before the
  change and restores them on stop. A dirty journal must be recovered before
  another system-proxy run.
- CA trust installation/removal is a separate explicit exact-SHA-256 operation.
  It can display an OS consent dialog and is never part of automated startup.
- Session pages contain at most the service-configured maximum (200 by
  default). Refresh notifications are lossy hints; the catalog query is always
  authoritative.
- Body retention remains metadata-only by default. Text, binary, missing,
  truncated, redacted, and lossy evidence are distinct inspector states.
- Interactive breakpoints use one same-build controller. Closing the window,
  disabling breakpoints, timing out, or losing the controller fails unresolved
  decisions closed.
- Composer replay requires explicit acknowledgement for non-idempotent methods
  and credential-bearing fields and uses the canonical Rust HTTP/TLS stack.
- Native `.tmcap` files are the streaming source of truth. JSONL export streams
  records sequentially. SAZ must be finalized and cannot preserve every native
  boundary or hook record, so each result includes a fidelity disclosure.
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
[product-state-and-support.md](product-state-and-support.md). The remaining
product phases cover Windows hardening/packaging and Linux qualification.
