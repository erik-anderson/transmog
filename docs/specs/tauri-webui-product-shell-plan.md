# Plan: Tauri and WebUI product shell

Status: active; Phases 0-8 completed and Phase 9 implemented on 2026-10-04;
signed clean-machine Phase 9 qualification remains pending
Audience: maintainers, product-shell authors, security reviewers, and release
engineers
Depends on: the completed application/session service, native capture, SAZ
export, automation, and WebSocket inspection
Initial platform: Windows with WebView2; Linux support follows a WebKitGTK
compatibility gate

## Mission

Build a production-grade desktop inspection tool without creating a second
proxy or session model. Tauri owns the native window and narrow operating-system
integration. Rust owns application state, validation, proxy/session lifecycle,
and server-side rendering. Microsoft WebUI compiles HTML templates into a
binary protocol and renders them in Rust. HTML and CSS are the default UI
languages; authored browser behavior is TypeScript and exists only in
interactive islands.

The shipped application must not require a Node.js process, bundle a browser,
or expose the proxy core directly to the WebView. The headless
`transmog-session` API remains independently usable by future commands and
embedding applications.

## Accepted technology direction

- Use Tauri 2 and its operating-system WebView. Windows uses WebView2. Linux
  uses WebKitGTK when that target is enabled.
- Start with Windows as the supported product target. Do not claim Linux
  support until the same functional, security, accessibility, and rendering
  contracts pass against a declared WebKitGTK baseline.
- Use Microsoft WebUI for build-time template compilation, Rust-native SSR,
  and Web Component islands. Do not add React, Vue, Svelte, JSX, a virtual DOM,
  or a JavaScript SSR process.
- Keep scriptless `.html` and `.css` components scriptless. Add a same-named
  `.ts` module only for event handling, browser APIs, local interaction state,
  or imperative accessibility behavior. Do not author JavaScript directly.
- Pin reviewed Tauri, WebUI, TypeScript, and bundler versions in Cargo and npm
  lockfiles. WebUI is currently a pre-1.0 dependency, so its API and generated
  protocol are contained behind a local adapter and upgrades are deliberate.
- Treat the Tauri command surface and WebUI render DTOs as same-build internal
  contracts. They remain breakable with atomic monorepo changes and do not
  stabilize experimental control v0.
- Ship all UI assets in the application. No CDN, remote page, analytics script,
  or runtime package download is allowed.

Primary references checked while writing this plan:

- [Tauri architecture](https://v2.tauri.app/concept/architecture/)
- [Tauri WebView versions](https://v2.tauri.app/reference/webview-versions/)
- [Tauri security configuration](https://v2.tauri.app/reference/config/)
- [WebUI architecture](https://microsoft.github.io/webui/guide/)
- [WebUI Rust handler](https://microsoft.github.io/webui/guide/integrations/rust/)
- [WebUI hydration](https://microsoft.github.io/webui/guide/concepts/hydration/)

## Layering

The intended dependency direction is:

```text
apps/desktop                 Tauri window, lifecycle, packaging
       |
transmog-app-webui        WebUI rendering and presentation DTOs
       |
transmog-app              product use cases and UI-neutral application state
       |
transmog-session          bounded live session and proxy lifecycle
       |
existing proxy libraries

transmog-host-windows     caller-owned Windows host adapter
       |
transmog-session::HostIntegration
```

`transmog-app` is reusable by a future CLI and owns product operations such
as selection-independent queries, replay requests, capture/export workflows,
settings validation, and redaction-safe diagnostics. It does not depend on
Tauri, WebUI, a WebView, or Windows.

`transmog-app-webui` maps application read models to bounded WebUI render
state and owns the loaded template protocol. It contains no OS mutation and no
proxy transport implementation. Rendering is independently testable without a
window.

`apps/desktop` is a thin Tauri binary. It registers the UI protocol, exposes a
small allowlisted command adapter, maps service delta hints to a bounded UI
notification channel, and coordinates graceful application exit.

`transmog-host-windows` implements transactional current-user system-proxy
and certificate setup. The application chooses whether to install it; neither
the session crate nor the renderer acquires OS side effects.

Lower layers must never depend on these product crates. Captured traffic,
filters, current selection, scroll position, and editor drafts must never
become Tauri global state required for proxy correctness.

## WebView delivery and update flow

The preferred production origin is a Tauri asynchronous custom URI protocol.
It lets the same Rust process serve WebUI-rendered documents, partials, CSS,
and compiled ESM assets without opening a TCP port. Initial implementation is
buffered SSR because correct initial paint and a small attack surface matter
more than progressive streaming for a local desktop shell.

Phase 0 must prove this path on WebView2 before it becomes architectural fact:

1. WebUI compiles one HTML-only component and one TypeScript island.
2. Rust loads `protocol.bin` once and renders the initial document.
3. The Tauri custom protocol returns correct content types, CSP, module, CSS,
   and cache headers.
4. Same-origin `fetch` or WebUI partial navigation works through the custom
   protocol.
5. The island can invoke one narrowly scoped Rust command and receive one
   bounded Rust-to-WebView notification.
6. Debug and packaged release builds work offline without a Node process.

If custom-protocol partial requests are not reliable, retain custom-protocol
documents and use typed Tauri commands for bounded JSON queries. Only if WebUI
cannot operate correctly through either route may the team consider an
ephemeral loopback HTTP server. That fallback requires an ADR, a per-launch
authentication design, origin and CSRF defenses, port-conflict tests, and a
shutdown proof. Tauri's localhost option is not the default because its own
documentation identifies additional security risk.

WebUI progressive streaming is deferred until a measured UI requires it and a
Tauri delivery path proves backpressure, cancellation, and CSP behavior. The
renderer interface must not preclude it.

The live-update flow is deliberately recoverable:

```text
session observer -> bounded catalog -> coalesced delta hint -> TypeScript island
       ^                                                    |
       +--------- authoritative bounded Rust query <--------+
```

Delta notifications carry only enough identity and revision information to
trigger a refresh. They are not authoritative state. The desktop adapter
coalesces bursts and applies a finite rate and queue. Subscriber lag causes a
page query, never an unbounded replay through IPC. Body chunks never cross the
Tauri boundary one event at a time.

## Rust and TypeScript boundary

Rust owns:

- `ApplicationSessionService` and every proxy/capture/controller handle;
- catalog queries, filtering, paging, and body-retention policy;
- request and response editor validation;
- breakpoint decisions and audit attribution;
- replay safety acknowledgements and executor selection;
- capture sealing and export orchestration;
- settings parsing, migrations, atomic persistence, and redaction;
- WebUI render state, CSP nonce generation, and response headers;
- all file dialogs, paths, OS mutations, logging, and diagnostics.

TypeScript owns only browser-local behavior:

- input, selection, keyboard, focus, resize, and scroll behavior;
- requesting a bounded command or authoritative refresh;
- presenting optimistic busy state while Rust decides;
- applying WebUI hydration and partial-update behavior;
- accessible interaction that cannot be expressed with HTML and CSS alone.

TypeScript may not decide whether a request is safe to replay, whether an
authority change is legal, whether a certificate is trusted, or whether an
edit is valid. Every command is revalidated in Rust. Generated JavaScript is a
build artifact and is never edited by hand.

The TypeScript toolchain stays small and app-local: npm with a committed
lockfile, TypeScript, the WebUI client package, and one pinned bundler. There is
no Node server at development or runtime; Node is only a build and browser-test
tool already required by the repository's Playwright gates.

## Security and privacy baseline

The main WebView is privileged application UI. Captured URLs, headers, bodies,
WebSocket messages, filenames, and imported capture data are hostile input.

- Navigate the main window only to the embedded application origin. Block
  arbitrary navigation, new windows, downloads, and remote subresources.
- Do not enable remote-domain IPC, a generic shell command, unrestricted file
  access, or a generic HTTP client. Define the minimum Tauri capability for
  each window and command.
- Use a restrictive CSP with no `unsafe-eval` and no general `unsafe-inline`.
  Pass a fresh nonce through `RenderOptions::with_nonce` for WebUI-generated
  scripts. Exercise Tauri's CSP injection and WebUI's generated markup together
  in packaged tests.
- Enable Trusted Types on Windows if Phase 0 proves the chosen WebUI navigation
  path is compatible. WebUI currently documents a limitation between enforced
  Trusted Types and client-side partial routing, so the plan must not claim
  both until the combination passes.
- Put untrusted values only in escaped WebUI bindings. Never feed captured data
  to raw HTML injection, `$webui` HTML fields, script URLs, CSS, import maps, or
  component metadata.
- Show captured HTML as escaped text initially. A rendered-page preview is a
  later security milestone and must use a separate sandboxed WebView with no
  Tauri IPC, storage, credentials, or network access.
- Keep body retention metadata-only by default. Enabling a finite body limit is
  an explicit user choice with clear memory and privacy consequences.
- Never expose CA private keys, ambient credentials, unredacted authorization
  fields, filesystem roots, or panic text to render state or logs.
- Return stable error categories and operator-safe messages. Preserve detailed
  causes only in bounded local diagnostics with explicit redaction.

## Windows host integration

The first supported adapter is current-user Windows integration. It is a
separate crate and implements the existing `HostIntegration` transaction.

- System-proxy apply records the exact prior state before modification and
  restore is idempotent with the same token.
- The adapter keeps its own small crash-recovery journal because the service's
  in-memory opaque token cannot survive process termination. Startup detects a
  dirty journal and offers exact restoration before starting another run.
- Certificate creation, installation, and removal are explicit user actions.
  The product creates one app-owned CA per installation/profile, protects its
  private key with current-user access controls, identifies it by exact public
  key or thumbprint, and never removes a similarly named certificate.
- Normal automated tests use fake registry and certificate-store backends.
  Opt-in Windows host tests use an isolated current-user fixture and always
  exercise rollback. UI consent dialogs remain manual smoke tests.
- Browser tests use the repository's durable development CA setup rather than
  disabling certificate validation. Production code never launches Chromium
  with certificate-error bypasses.

The first app slice may require manual browser proxy configuration while the
Windows adapter is under construction. That does not weaken TLS verification
or justify embedding host mutation in the proxy core.

## Product behavior and UX constraints

The first shell is keyboard-usable and accessible from the beginning, not as a
final polish phase. Native HTML semantics, visible focus, reduced-motion
support, high-contrast themes, scalable text, and screen-reader names are exit
criteria for every interactive component.

The session grid is windowed or paged over authoritative catalog queries. It
does not materialize the complete configured catalog in DOM or JavaScript.
Selection and filters survive lossy refresh hints, while evicted sessions are
shown as unavailable rather than reconstructed from stale UI data.

Inspectors distinguish original, client-boundary, upstream-boundary, and final
evidence. They show truncation, redaction, decoding, loss, and terminal failure
explicitly. Binary bodies have bounded text, hex, and metadata views; syntax
highlighting is optional and must never require executing captured content.

Breakpoint editors show the stable hook attribution that will be recorded.
Invalid edits remain local drafts and cannot be submitted. Continue, replace,
and abort actions are phase-specific. Closing the window or losing the
controller fails the affected paused exchange according to the service's
fail-closed contract; it never leaves traffic suspended indefinitely.

## Implementation phases

### Phase 0: Dependency and delivery spike

Status: complete. ADR 0007 records the delivery decision and the checked-in
spike lives under `apps/desktop`. The WebView2 debug and optimized release
smoke tests exercised WebUI SSR and hydration, custom-protocol module/CSS/JSON
delivery, Trusted Types, CSP, a typed Tauri command, and one channel hint with
no browser errors or CSP violations. The NSIS package was reproduced with
Cargo offline and network access blocked after Tauri's hash-verified packaging
tools had been cached.

Create the smallest Tauri/WebUI application described in the custom-protocol
proof above. Record exact versions, licenses, transitive dependency changes,
WebView2 runtime expectations, and the result of custom-protocol fetch,
hydration, CSP, Trusted Types, Tauri command, and notification tests. Compare
the installed toolchain with the repository's Windows LLVM/Ninja profile.

Exit gate: an ADR selects custom-protocol documents plus either WebUI partials
or bounded Tauri query commands. The packaged Windows spike runs offline and
contains no development server.

### Phase 1: Application facade and render foundation

Status: complete. `transmog-app` owns the UI-neutral facade and bounded status
contract, `transmog-app-webui` owns the loaded protocol and secure renderer,
and the Tauri binary is limited to custom-origin delivery and allowlisted
commands. The checked-in asset task rejects stale TypeScript output, renderer
tests run without Tauri, and window close performs bounded service shutdown
before destroying the main window.

Add `transmog-app`, `transmog-app-webui`, and the thin desktop binary.
Define bounded presentation DTOs with opaque IDs/cursors and stable error
categories. Add a Rust asset build task that runs the pinned TypeScript bundle
first, consumes its WebUI projection manifest, compiles templates, validates
all generated artifacts, and fails on stale output. Load the protocol once.

Create the shell layout, status surface, diagnostics region, theme tokens, and
accessible navigation as HTML/CSS-first WebUI components. Establish graceful
window close: request service stop, wait for bounded drain and host restore,
then exit or present a retryable failure.

Exit gate: renderer and app facade run without Tauri in Rust tests; debug and
release desktop builds use identical generated assets and a strict CSP.

### Phase 2: Proxy lifecycle and Windows setup

Status: complete. The application facade owns validated bind/start/stop/retry
operations and explicit create-new CA generation. `transmog-host-windows`
provides a caller-owned current-user adapter with exact state snapshots, a
durable recovery journal, rollback on partial apply, idempotent restoration,
and explicit exact-thumbprint certificate-store actions. The desktop supports
manual proxy configuration and opt-in system configuration; automated tests
use an injected fake backend and never touch the registry or trust store.

Expose start, stop, status, listener endpoint, failure, and recovery actions.
Implement the Windows host adapter, explicit CA workflow, exact proxy restore,
crash journal, and first-run prerequisites. Keep a manual-configuration mode
for developers and embedders.

Exit gate: start-twice, concurrent stop, window close, failed restore, crash
recovery, and user cancellation are deterministic. Fake-host tests cover all
branches and an opt-in Windows smoke test restores the exact prior state.

### Phase 3: Live session browser

Status: complete. The application exposes capped authoritative queries with
validated filters and a bounded opaque-cursor registry. Rows include protocol,
status, timing, byte counts, capture state, terminal state, and per-exchange
sequence loss. The desktop coalesces lossy hints for 75 ms and always refreshes
by query; a 10,000-session test exercises the configured catalog maximum.

Render a bounded paged table with method, host, path, protocol, status,
duration, byte counts, terminal state, capture state, and visible loss markers.
Add server-side filters, opaque cursor navigation, selection, burst-coalesced
delta hints, and recovery after lag. Do not retain a duplicate session graph in
TypeScript.

Exit gate: 10,000 configured sessions, high event rates, catalog eviction,
subscriber lag, and service restart remain responsive within recorded CPU,
memory, DOM-node, render, and IPC budgets.

### Phase 4: Inspectors and safe body viewing

Status: complete. Bounded inspector DTOs preserve explicit observation
boundaries, duplicate headers, trailers, routes, hook attribution,
diagnostics, terminal state, and WebSocket evidence. Arbitrary bytes use an
explicit text/hex/missing representation with truncation and loss flags.
Captured values reach the DOM only through `textContent`; hostile and binary
unit tests guard this contract.

Add request/response overview, original and effective heads, trailers, route
attempts, hook effects, diagnostics, timing, content-coding information,
WebSocket evidence, and bounded body views. Make truncation, redaction, missing
evidence, decode failure, and binary data impossible to confuse with complete
plain text.

Exit gate: hostile HTML, control characters, very long fields, invalid Unicode
boundaries, binary bodies, nested encodings, and redacted credentials render
without script execution, layout breakage, or unbounded allocation.

### Phase 5: Break and modify

Status: complete. The application owns an exclusive same-build controller,
finite phase and body settings, a bounded paused-decision map, stale-action and
phase validation, and continue/abort/head/body replacement. Disabling,
shutdown, queue loss, timeout, and controller loss drop single-use reply
capabilities and therefore use the transport's fail-closed behavior. Effects
retain the stable `transmog.session.interactive-control` hook identity.

Attach the exclusive controller through the application facade. Add explicit
phase enablement, paused-exchange state, deadlines, continue, abort, head edit,
and decoded body replacement. Present audit attribution and validate every edit
again in Rust.

Exit gate: controller loss, saturation, timeout, stale actions, invalid edits,
window closure, and simultaneous paused exchanges all follow the core's typed
fail-closed and isolation contracts.

### Phase 6: Composer and replay

Status: complete. A structured composer validates absolute targets, headers,
hex or UTF-8 bodies, credential acknowledgement, and non-idempotent risk in
Rust. Desktop replay uses the same canonical `transmog-http` TLS-verifying
origin client and route policy family rather than WebView networking. Replay
still passes through the session service's cancellation, timeout, and response
bounds and records bounded redaction-safe local history.

Add a structured request composer over `ReplayRequest`. Require visible
acknowledgement for non-idempotent requests and credential-bearing headers.
Execute through the application's normal route and upstream policy, never a
WebView fetch or a second networking stack.

Exit gate: authority validation, credentials, cancellation, timeout, response
bounds, history drafts, and replay attribution have deterministic Rust and
end-to-end tests.

### Phase 7: Capture, import, and export

Status: complete. The application exposes quota-bound streaming native capture,
status, sealing, bounded native recovery/import, create-new streaming JSONL,
and finalized strict or extended SAZ export. Interrupted native tails recover
their valid prefix; failed exports remove only the newly created partial file;
SAZ results disclose incomplete/skipped sessions and the evidence the format
cannot preserve.

Add native streaming capture start/stop/status, quota selection, failure
reporting, and finalization. Add streaming JSON-lines export and finalized SAZ
compatibility export. Imports and future project storage stay above the session
service and operate under explicit size and path limits.

Exit gate: capture remains valid after app interruption, partial native files
recover their valid prefix, existing destinations are never overwritten, and
SAZ fidelity disclosures reach the UI and exported result.

### Phase 8: Product state and operational diagnostics

Status: complete. `transmog-app` owns a bounded v2 product-state schema,
explicit v1 migration, validated atomic generations, corrupt-generation
fallback, and non-fatal persistence. It also owns a redacted 256-event
diagnostic ring, a best-effort 1-MiB rotating JSON-lines sink, native runtime
reporting, and create-new support bundles whose default contents omit captured
traffic, credentials, private keys, and paths. The Windows shell persists
window geometry on close without putting persistence on the shutdown critical
path.

Persist only versioned product preferences, window state, recent artifact
references, and explicit privacy settings. Use atomic replacement, schema
validation, bounded migrations, and corrupt-file recovery. Do not persist live
control envelopes or treat the settings file as a capture database.

Add bounded structured logs, copyable redacted diagnostics, dependency/version
reporting, WebView version reporting, and a support bundle that excludes bodies,
credentials, and private keys by default.

Exit gate: upgrades, corrupt settings, read-only directories, disk exhaustion,
and support-bundle generation cannot prevent proxy shutdown or leak secrets.

### Phase 9: Windows hardening and packaging

Status: implementation complete. The Windows shell now has first-registered
single-instance enforcement, explicit update and uninstall maintenance modes,
fail-closed proxy recovery, exact SHA-256 certificate ownership, conservative
app-data cleanup, a current-user NSIS bundle, and signed/offline packaging
policy. Evergreen WebView2 is an assumed host prerequisite and no runtime,
bootstrapper, or installer is bundled. Local packaged-WebView accessibility,
high-contrast, high-DPI, long-label, startup, memory, and soak checks pass.
The release exit gate remains open until a human supplies a signing identity
and records the signed clean-machine checklist, including the manual trust
dialogs.

Produce a signed-release-ready Windows bundle with WebView2 prerequisite
handling, installer/uninstaller policy, single-instance behavior, clean update
handoff, crash recovery, and explicit ownership of app data and certificates.
Run accessibility, high-DPI, high-contrast, keyboard, localization-length,
startup, memory, and long-running soak tests.

Exit gate: a clean Windows machine can install, run offline, intercept verified
traffic, modify a request, capture/export it, restore host state, and uninstall
without leaving the system proxy enabled or removing unrelated trust material.

### Phase 10: Traffic inspection, automation, and sandboxed scripting

Build the production traffic workspace, default-on quota-bound response body
retention, original/effective multi-representation inspectors, conditional
native rules, saved-response autoresponders, sandboxed TypeScript/JavaScript,
Monaco authoring, and safe image preview described by the dedicated
[traffic inspection, automation, and sandboxed scripting plan](traffic-inspection-automation-scripting-plan.md).

Exit gate: native rules and scripts produce the same typed Hooks v2 actions,
script failures abort visibly, circular body eviction preserves metadata, and
captured content cannot execute in the application origin.

### Phase 11: Linux qualification

Install Tauri's documented WebKitGTK prerequisites in the local Linux/Docker or
WSL test environment. Verify every used Web Platform feature rather than
assuming WebView2 parity. Prefer standard Web Components, Light DOM fallback,
and capability probes over browser-family forks.

Exit gate: the declared WebKitGTK baseline passes the same render, CSP,
security, accessibility, lifecycle, and end-to-end contracts. Until then Linux
builds are experimental and Windows remains the only supported desktop target.

## Build and dependency policy

- Preserve the repository's Windows LLVM/Ninja native profile and Windows SDK
  prerequisite. The Rust target name may continue to use the Windows MSVC ABI.
- Add Tauri/WebUI prerequisites to `docs/building.md` before requiring them.
- Keep `Cargo.lock` and the app-local npm lockfile committed. Use `npm ci`, not
  floating installs, in repeatable builds.
- Generate WebUI protocol, CSS, ESM, projection manifest, and Tauri resources
  from one top-level build command. A clean checkout must not depend on stale
  generated files from a developer machine.
- Verify whether generated assets should be checked in during Phase 0. If they
  are checked in, a reproducibility test regenerates and diffs them. If they are
  not, every release path builds them before Cargo packages the app.
- Extend license/advisory policy to Rust and npm production dependencies.
  Record the WebView runtime as a platform prerequisite rather than a bundled
  dependency.
- Hosted automation remains inactive until a human explicitly enables it.

## Test strategy

The app adds layers of tests without weakening existing proxy gates:

- pure Rust unit and property tests for application commands, DTO bounds,
  state transitions, settings migrations, and hostile display data;
- deterministic WebUI protocol/render tests and semantic HTML snapshots with
  normalized nonces, never broad pixel snapshots as the primary oracle;
- TypeScript type checking and focused tests for authored islands only;
- browser component tests against WebUI output for hydration, focus, keyboard,
  partial refresh, CSP violation absence, and accessibility;
- Tauri adapter tests for capability scope, custom-protocol routing, command
  validation, notification lag, and graceful close;
- packaged WebView2 end-to-end tests using standalone origins and the real proxy
  for HTTP/1.1, HTTP/2, HTTP/3 egress, every supported content encoding,
  WebSocket traffic, breakpoint edits, replay, and capture/export;
- opt-in Windows host tests that can trigger user consent, separated from the
  non-mutating default test gate;
- fuzz/property targets for command deserialization, render DTOs, captured
  hostile text, settings, import manifests, and custom-protocol paths;
- bounded load and soak tests with explicit CPU, memory, DOM, IPC, startup,
  shutdown, and artifact-size measurements;
- the existing strict Clippy, dependency policy, one-BoringSSL, standalone
  interoperability, live Chromium, and local Linux matrices.

Every end-to-end test must prove that the proxy actually observed or modified
the exchange so a direct client path cannot produce a false pass. Certificate
validation stays enabled.

## Definition of done

- [x] Tauri/WebUI delivery ADR and offline packaged spike pass on WebView2.
- [x] Layering prevents Tauri, WebUI, and platform APIs from entering proxy
      libraries or `transmog-session`.
- [x] Rust owns authoritative state, validation, rendering, and product
      operations; authored browser code is TypeScript-only and minimal.
- [x] Custom protocol, CSP, Trusted Types decision, navigation, and Tauri
      capabilities pass a security review.
- [x] Proxy lifecycle and exact Windows host restoration survive normal exit,
      failed stop, restart, and crash recovery.
- [x] Live session browsing remains bounded and recovers after every visible
      loss mode.
- [x] Inspectors safely represent redaction, truncation, binary content,
      content codings, WebSocket evidence, and audit attribution.
- [x] Breakpoint edits and replay use existing service/core paths and their
      fail-closed safety contracts.
- [x] Native capture, streaming export, and SAZ compatibility export work from
      the application without making the UI authoritative.
- [ ] Windows accessibility, packaging, offline startup, soak, and clean-machine
      end-to-end gates pass.
- [ ] The traffic workspace, conditional native automation, autoresponse,
      response-body retention and views, sandboxed scripting, Monaco authoring,
      and safe preview plan passes its definition of done.
- [x] Build prerequisites and dependency/supply-chain procedures are checked in.
- [x] Existing core, interoperability, fuzz, and Linux gates remain green.
- [x] Hosted CI remains inactive until a human explicitly enables it.
- [x] Linux is labeled experimental until its WebKitGTK qualification gate
      passes; no release claim is inferred from successful compilation alone.

## Deliberate exclusions

The first product-shell milestone does not include multi-user remote control, a
stable external control protocol, cloud sync, collaborative sessions, plugin
execution, arbitrary captured-page execution, mobile targets, reverse-proxy
management, or advanced tunnels. Those require separate threat models and
plans. The same human-only stabilization trigger for control v0 remains in
force.
