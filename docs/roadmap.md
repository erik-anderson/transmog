# Roadmap

This roadmap lists remaining work. Current behavior is documented in the
[topic guides](README.md).

Priorities are intentionally not tied to dates. A feature moves out of this
file when its durable contract is documented and its implementation and tests
are checked in.

## Desktop and traffic workspace

- Add a searchable capture library and project-level organization across saved
  captures, persisted automation, scripts, response assets and preferences.
- Add desktop file actions for the implemented bounded rule/asset interchange,
  conflict review and disabled-import APIs in [portable investigations](portable-investigations.md).
- Add request-body auto-response conditions only with explicit retention,
  privacy, resource, and replay semantics.
- Extend headless tooling for persisted automation and script-workspace
  management. Guided support recording, circular capture, native streaming
  output and password-protected export already exist in the CLI.

### Format converter

**Status:** planned; the [toolbar icon](../apps/desktop/icons/format-converter.svg)
is checked in. This plan does not enable the tool yet.

Help users decode, encode or hash text while inspecting traffic without losing
their place. Today, the top bar contains the brand and connection controls;
headers support selection/copying, and expanded editors are in-window dialogs.
The converter adds a utility available in the main window and capture viewers,
independent of proxy state. V1 includes common encodings and hashes. Drafts stay
in memory while their converter is open; they are not restored after restart.
Compression and custom formats through TypeScript extensions are deferred.

#### Entry points and layout

- Add a **Format converter** button to the top bar, beside the connection/tools
  area. Use the checked-in icon with a visible label where space allows and an
  accessible name and tooltip in compact layouts. Preserve room for proxy state
  and its primary action, including when an update action is visible.
- Open a nonmodal flyout anchored to that button. It contains an editable Input,
  a grouped Conversion selector, a read-only Output, and **Copy output**, **Use
  output as input**, **Open in new window**, and Close actions. Input and Output
  remain visible together in a vertical stack; each scrolls independently.
  Output-as-input is an explicit, undoable edit for chaining conversions.
- Start with empty input and **Choose a conversion**. Do not infer a transform
  or read the clipboard automatically. Reopening the button restores its draft,
  transform, selection and scroll. Compute after an explicit transform choice;
  subsequent edits update the result after a short debounce.
- Use a native popover with named nonmodal dialog semantics, not menu semantics
  for the editor controls. Clamp it to the viewport, keep actions reachable at
  800×600 and 200% zoom, and reflow without hiding either editor. Use CSS anchor
  positioning with a fallback that remains beside the trigger on supported
  WebView2 versions; verify the fallback rather than assuming support.
- Escape, Close and clicking outside dismiss the flyout without clearing its
  draft. Escape/Close restore the invoker's focus; outside clicks retain their
  destination focus. Traffic selection, scrolling and workspace drafts remain
  intact. The flyout is a utility, not a new primary workspace.

#### Context actions

- Add **Open in Format converter** to a custom context menu wherever users can
  select text in trusted application UI: ordinary text, full URLs, inputs,
  textareas, editable content, inspector bodies, reports and Monaco editors.
  Keep normal Copy/Cut/Paste and existing contextual actions available.
- Snapshot the selected text before opening the menu changes focus. Support
  native selection ranges, input selection offsets, shadow roots and Monaco's
  selection API. Use the clicked selection, not an unrelated selection left in
  another pane. Preserve whitespace and line endings; treat whitespace-only
  selections as input. Support Shift+F10 and the keyboard Menu key.
- Right-clicking a request/response header row without a relevant text selection
  uses that occurrence's complete value, without its name or separator. A text
  selection at the clicked location takes precedence. Preserve duplicate header
  occurrences; do not join them or convert shortened display text. Fetch the
  exact value where necessary, and explain unavailable/redacted/binary values
  instead of converting a placeholder. Empty header values remain valid input.
- The action opens the host window's anchored flyout with the snapshot as input.
  If replacing a nonempty draft, offer **Replace input**, **Open in new window**,
  and **Cancel** before changing it. Replace participates in input undo. Cancel
  restores the source focus and selection. Conversion never changes source
  traffic, headers or editor text.
- Give standalone converters the same selection action, opening another
  independent converter window. Keep captured-page previews isolated; their
  untrusted browser content must not gain a bridge into application commands.

#### Independent windows and draft ownership

- **Open in new window** detaches the current draft into a resizable native
  window titled **Format converter N — Transmog**, using unique session/window
  IDs. There is no fixed product limit on the number of open converters. Each
  owns its input, operation, output, undo state and work cancellation.
- Transfer the draft only after the destination initializes successfully. Freeze
  or revision-check the handoff so typing during creation cannot be lost. Failed
  creation leaves the original flyout usable with its complete draft and a retry
  action. Successful handoff closes the flyout; its next use starts a new draft.
  Do not implement this with the existing modal `editor-surface` expansion.
- Detached converters remain usable if the originating capture viewer or main
  window closes. Opening or closing a converter never controls the proxy. Keep
  normal main-window proxy shutdown behavior and include converters in explicit
  Quit/update coordination. Escape does not close a standalone window.
- Dismissing a flyout preserves it; explicitly closing a standalone converter
  ends that scratch session. Quit/restart clears all converter drafts. Do not
  store contents in workspace preferences, browser storage, window titles,
  URLs, diagnostic logs or telemetry. Copy/save remain deliberate user actions.

#### Conversion contract

| Group | V1 operations |
| --- | --- |
| Base64 | Encode/decode standard Base64 and Base64url |
| URLs | Encode/decode a URL component; separately encode/decode form values where `+` means space |
| Hex | Bytes to/from hexadecimal; accept explicit ASCII whitespace between complete byte pairs |
| HTML | Encode/decode character references as inert text |
| Strings | Quote/unquote a JSON string, including its escape sequences |
| Hashes | MD5, SHA-1, SHA-256, SHA-384 and SHA-512; lowercase hexadecimal output |

Use UTF-8 by default for conversions between text and bytes. Put explicit
UTF-8, UTF-16LE, UTF-16BE and Windows-1252 choices in an encoding disclosure;
never silently replace unrepresentable characters or malformed sequences.
Distinguish byte output from text. Offer Text/Hex representations for input and
output, starting with Text input and keeping Hex available when bytes cannot be
decoded losslessly. **Use output as input** transfers the result's kind and bytes,
switching to Hex input for binary output; it must not accidentally hash the text
of a hex dump. Text encoding affects byte conversion and hashing, not the
URL/HTML/JSON escaping definitions. Hashes are one-way; do not offer a reverse
action or imply that a digest is encryption.

Define padding, accepted whitespace, malformed escapes, BOM handling and empty
input behavior for each operation before implementing it. Preserve original
input; do not trim or silently repair it. Show validation failures beside the
input, distinguish them from an empty result, and disable copying/chaining stale
output while a newer conversion is pending or invalid. **Copy output** copies
the selected text/hex representation; a secondary **Save output…** writes the
complete result bytes through the existing native save-dialog pattern. Saving
as traffic, files as input, compression and automatic format detection are out
of scope for v1.

Run work off the UI thread, with revision IDs and cancellation so late results
cannot overwrite newer input or steal focus. Start with an explicit 8 MiB input
byte limit and 32 MiB output byte limit per conversion; reject excess rather
than silently truncate. Validate those proposed limits against expansion and
native memory measurements before shipping. Bound worker concurrency and
queued work across windows without imposing a converter-window count limit.

#### Implementation sequence

1. **Contracts and conversion engine.** Add a UI-independent conversion service
   under `transmog-app`, reusing existing byte/encoding/hash primitives where
   their behavior fits. Define typed operation IDs, text/byte inputs, outputs,
   validation errors, limits and cancellation. Establish known-answer and
   malformed-input tests. Keep an internal operation catalog; do not publish
   an extension API or execute user scripts in v1.
2. **Shared editor and anchored flyout.** Add WebUI template/CSS/TypeScript
   components for the conversion surface and host. Extend `app-shell` and shared
   initial state; follow the installed WebUI reference and existing editor
   lifecycle rules. Wire the canonical icon as an inline decorative SVG with
   inherited color. Preserve drafts independently of component visibility.
3. **Native window support.** Extend `window_workspaces.rs`, desktop dispatch,
   bootstrap/rendering and window cleanup with an explicit converter role.
   Current non-main-window assumptions describe capture viewers and cannot be
   reused unchanged. Give converters only their conversion, handoff, clipboard,
   save and window lifecycle capabilities; they need no traffic catalog or
   proxy/automation permissions. Bind in-memory handoff tokens to the intended
   window, acknowledge receipt and release abandoned handoffs and work.
4. **Context-menu integration.** Add a shared selection/context action, then
   adapters for native text controls, Monaco and `message-inspector` header
   rows. Use typed bubbling events across workspace/shadow boundaries. Test
   event precedence against traffic row and editor menus; do not install a
   blanket right-click handler that disables unrelated actions.
5. **Qualification and documentation.** Run the UX routine and required UI,
   Rust and native checks. Document shipped behavior in the desktop and traffic
   guides, add any durable convention to UX principles, and retire this plan.

#### Icon and acceptance checks

The canonical icon pairs a drawn **A** and **#** with opposing arrows to suggest
text and encoded representations. It is intentionally distinct from the
Traffic icon's arrows. Use a 24×24 view box, 1.75-unit rounded strokes, no fill
and `currentColor`, matching existing utility icons. Render at 20 CSS pixels in
the toolbar, with a normal button hit target. Keep the icon decorative inside
the named button; do not use a font glyph, raster image or theme-specific copy.

- Verify exact encode/decode round trips, published digest vectors, empty and
  whitespace-only input, Unicode/emoji, invalid bytes and escapes, binary output,
  limits, cancellation and results arriving out of order.
- Exercise the production templates in light, dark and forced colors at
  1440×900, 960×720 and 800×600, plus zoom. Check keyboard traversal, menu keys,
  Escape, focus return, long values, scrolling and accessible names.
- Cover selected text across all supported surfaces, context actions on both
  header columns, duplicate/empty/redacted headers, selection versus row-value
  precedence, draft replacement, copy, save cancellation and output chaining.
- Native WebView2 checks must open several converters, edit them independently,
  create more after closing some, detach during pending work, simulate window
  creation failure, close the origin, and verify restart clears contents.
  Confirm converter roles cannot invoke proxy/capture/automation commands.
- Run `npm run check` and `npm run test:workspaces`, the scoped strict Clippy and
  Cargo tests in the [UI instructions](../apps/desktop/ui/AGENTS.md), and affected
  checks through `scripts/test-windows-desktop.ps1`. Use the portable Python
  environment check before builds. Keep screenshots and test reports in
  generated evidence. The present plan/icon-only change needs document, SVG
  and visual review; application qualification belongs to implementation.

## Capture and inspection fidelity

- Broader case/rule CLI validation, matching and extraction remain deferred under
  the revision-0 [portable investigation contract](portable-investigations.md), along with
  offline script execution, persisted browser runs and CLI comparison (K5).
  Desktop case previews and saved recorded-request/capture comparisons are implemented.
  Broader project organization and any future format stability decision remain
  separate from the experimental live control protocol.
- Expand safe previews or semantic viewers only after a format-specific parser,
  sandbox, CSP, resource-limit, and hostile-corpus review.

## Platforms and distribution

- Extend platform qualification beyond the Linux container gate to native Linux
  and macOS host integration, networking and process sandboxes.
- Decide whether a Linux or macOS desktop shell has a supportable system WebView
  and process-sandbox story before enabling those products.
- Add macOS caller-process attribution only if it can use a supported and
  testable operating-system contract; the available `libproc` socket APIs are
  currently private and subject to change.
- Review activation of the deferred hosted platform, browser, fuzz, and
  performance workflows in the [CI guide](../ci/README.md).

## Protocol expansion

- HTTP/3 WebSockets.
- Application-owned upstream byte streams for upgraded connections.
- Native downstream HTTP/3, CONNECT-UDP, MASQUE, and WebTransport.
- Reverse-proxy listener products, including certificate provisioning, load
  balancing, and health checks. Existing provider seams do not imply these
  features.
- QUIC 0-RTT and connection migration only after replay and identity policy is
  explicit.

## Chromium transport alignment

The built-in profiles target Chromium 154.0.8037.99. Chromium 155 uses the same
wire targets covered here. Use the [local comparison harness](chromium-comparison.md)
to qualify changes; a matching JA4 hash alone does not establish transport or
certificate-verification equivalence.

Current offers and connection policy are documented in
[Chromium negotiation](chromium-negotiation.md), [ALPS](alps.md), and
[certificate verification](chromium-verifier.md). Dependency patches and their
reproduction commands live in the [transport inventory](../third_party/rust-transport/README.md).

### Expand comparison coverage

The harness compares fresh GET requests over H1.1, H2 and H3. Extend it to cover
connection reuse, HEAD/POST, duplicate headers and cookies, edits and reroutes,
accepted ALPS, and resumption. Keep protocol fallback and missing evidence as
hard errors. Compare randomized ECH padding and QUIC GREASE across samples;
a single random value difference is not a compatibility defect.

### HTTP/2 startup ordering

The initial SETTINGS values, header order, priority and HPACK output match the
target in the fresh-request case. A real SETTINGS ACK still precedes the initial
WINDOW_UPDATE and HEADERS. A peer-settings bootstrap API must preserve flow
control, concurrency limits and ALPS settings before request allocation. Prefer
an upstreamable encoder/connection API; do not rewrite encoded HPACK bytes.

Qualify uploads, trailers, connection reuse and extended CONNECT alongside any
startup-order change.

### HTTP/3 dynamic QPACK

The pinned quiche decoder has incomplete dynamic-table paths. Keep advertised
capacity and blocked-stream limits at zero until bounded updates, insertions,
evictions, blocked streams, acknowledgements, cancellation and malformed-input
handling work against an independent peer. Prefer upstream support; assess a
maintained implementation or adapter migration if necessary.

### HTTP/3 datagrams

H3_DATAGRAM and the QUIC datagram transport parameter remain absent. Supporting
them requires an application consumer, routing and ownership rules, capture
semantics and bounded queues. Review that product scope before implementation.
HTTP/3 extended CONNECT and WebTransport likewise need working end-to-end behavior
before their capabilities can be advertised.

For each change, run the affected interoperability tests and local comparison.
Keep qualification reports and review transcripts in generated evidence, not in
the roadmap.

## Post-quantum trust

- Evaluate PQ interception issuers and Merkle Tree Certificate support as
  Chromium's PKI requirements develop. Define trust installation, protected key
  storage, migration and rollback before changing the interception root.
  Current hybrid exchange and ML-DSA handshake support are documented in the
  [certificate model](chromium-negotiation.md#algorithms-and-clienthello).

## Control protocol stabilization

Control revision 0 is a same-build, breakable internal contract. It may remain
that way indefinitely. Stabilization begins only when a human maintainer
explicitly records that it now makes sense; elapsed time, repository splits,
consumer count, or test coverage do not trigger it.

Once triggered, stop protocol feature work long enough to inventory producers,
consumers, persisted references, and deployment topologies; define compatibility
and support windows; specify canonical serialization and unknown-field behavior;
add conformance and cross-version fixtures; and record the v1 decision in an
ADR before promising compatibility.

## Repository boundaries

The monorepo keeps cross-layer changes atomic while APIs are evolving. Split a
crate into another repository only when independent ownership, release cadence,
security review, or distribution warrants the operational cost. A split must
retain acyclic dependency direction, runnable contract tests, explicit release
coordination, and clear license/security ownership. It does not stabilize the
control protocol by itself.
