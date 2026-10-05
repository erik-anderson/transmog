# ADR 0008: Traffic workspace and script isolation

Status: accepted

## Context

Transmog needs Fiddler-style traffic inspection and automation without moving
durable product state or a JavaScript runtime into the proxy core. Built-in
rules and scripts must produce the same auditable Hooks v2 actions. Response
bodies are potentially sensitive and hostile, while locally-authored scripts
can still contain mistakes or compromised dependencies.

## Decision

1. The proxy core gains only a finite per-exchange registration-provider
   contract. Product configuration remains outside the core. Each admitted
   exchange receives an immutable registration snapshot, and every rule or
   script keeps its own stable interceptor identity.
2. Response bodies are retained by a product-layer store. The default is a
   one-GiB circular cache for response boundaries; request bodies remain off by
   default. Metadata survives body loss, truncation, and eviction.
3. Built-in rules are the preferred path for common conditions and mutations.
   Scripts use the same validated action vocabulary for advanced decisions.
4. JavaScript runs in a dedicated `transmog-script-host` process, normally one
   process per active script. The host has no filesystem, network, environment,
   process, UI, registry, clipboard, timer, WebAssembly, dynamic import, or
   string-code-generation capability.
5. V8 limits are defense in depth. Windows Job Object and restricted-token or
   AppContainer policy form the operating-system containment boundary. The
   supervisor owns authenticated bounded IPC, deadlines, termination, and
   fail-closed recovery.
   The implemented Windows boundary uses an ephemeral, zero-capability
   AppContainer per host plus a kill-on-close Job Object with one-process,
   memory, and UI limits. A trusted bootstrap creates the child suspended,
   assigns the Job Object, clears the environment to the structural Windows
   variables required by `CreateProcessW`, and only then resumes it. The child
   verifies both boundaries before reading the authenticated pipe.
6. A script error aborts the affected exchange. An invalid saved revision never
   replaces the last valid active revision. Diagnostics identify the script,
   revision, phase, source location, and resource failure without including
   credentials or body bytes.
7. Captured content never executes in the application origin. Text and byte
   views are bounded and escaped. Image decoding occurs in a separately
   restricted worker before the WebView receives raster output.
8. Monaco is a locally-packaged authoring component. Rust-side validation and
   the script host are authoritative; Monaco never executes traffic scripts.

## Consequences

- Headless applications and a future CLI can reuse the workspace and script
  supervisor without Tauri, WebUI, or Monaco.
- Dynamic rules can change without restarting the listener while in-flight
  exchanges remain deterministic.
- Script activation consumes a finite process slot and fails atomically when
  isolation or capacity is unavailable.
- Body retention requires explicit quota, cleanup, range-read, and loss-state
  APIs rather than growing the in-memory session catalog.
- The V8 binary and Monaco assets increase build and package size and therefore
  require exact pins, reproducible caching, license evidence, and offline build
  tests.
- `deno_core` is exactly pinned to 0.412.0 and V8 to 150.4.0. The reviewed
  Windows V8 archive has an independently checked SHA-256 and a cache-priming
  script for offline release builds. A custom product startup snapshot is not
  used yet: the measured host initialization is already small relative to one
  process per active script, while each user module is necessarily dynamic.

## Rejected alternatives

- A single opaque product dispatcher was rejected because existing hook audit
  attribution is registration-based and would hide the rule or script that
  changed traffic.
- Running scripts in the Tauri WebView was rejected because it would mix
  captured hostile content, editor state, and traffic authority.
- Running V8 in the proxy process was rejected because an isolate alone is not
  the intended security boundary for untrusted code.
- Making every header rule a script was rejected because it adds IPC latency,
  runtime availability, and a larger failure surface to common deterministic
  operations.
- Persisting decoded duplicates of every body was rejected because it doubles
  sensitive storage and quota pressure; decoded representations are derived on
  demand under content-processing limits.
