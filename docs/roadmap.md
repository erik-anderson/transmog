# Roadmap

This document contains only work that is not yet part of the supported
implementation. It does not repeat completed phases; current behavior is
documented in the topic guides linked from the [documentation index](README.md).

Priorities are intentionally not tied to dates. A feature moves out of this
file when its durable contract is documented and its implementation and tests
are checked in.

## Desktop and traffic workspace

- Add measured list virtualization when retained traffic volume demonstrates
  that the current bounded, non-paginated list needs it. Preserve live-tail,
  pinned-selection, keyboard, and accessibility behavior.
- Add a durable indexed capture library and project-level organization above
  the finite live-session catalog and existing persisted automation, scripts,
  response assets, and product preferences.
- Add richer structured editing for cloned response headers, plus bounded
  import/export and conflict handling for automation collections.
- Add request-body auto-response conditions only with explicit retention,
  privacy, resource, and replay semantics.
- Add bounded URL regular-expression matching as an explicit alternative to
  exact URL matching. First-enabled-match ordering and audit attribution must
  remain unchanged.
- Build a user-facing command-line application workflow over `transmog-app` for
  unattended capture, automation, and native streaming export. The existing
  CLI remains the lower-level proxy and capture utility.

## Capture and inspection fidelity

- Add native streaming WebSocket message/control records. The current runtime
  exposes bounded terminal WebSocket evidence but does not persist a complete
  message stream in TMCap.
- Define durable project and capture-index migrations separately from the
  experimental live control protocol.
- Expand safe previews or semantic viewers only after a format-specific parser,
  sandbox, CSP, resource-limit, and hostile-corpus review.

## Platforms and distribution

- Qualify the portable libraries on supported Linux and macOS hosts, including
  the real networking and sandbox-relevant test suites rather than compile-only
  checks.
- Decide whether a Linux or macOS desktop shell has a supportable system WebView
  and process-sandbox story before enabling those products.
- Add macOS caller-process attribution only if it can use a supported and
  testable operating-system contract; the available `libproc` socket APIs are
  currently private and subject to change.
- Complete signed Windows release qualification on a clean machine for each
  release candidate, and explicitly review activation of the broader hosted
  platform, browser, fuzz, and performance workflows.

## Protocol expansion

- HTTP/2 extended CONNECT and HTTP/3 WebSockets.
- Application-owned upstream byte streams for upgraded connections.
- Native downstream HTTP/3, CONNECT-UDP, MASQUE, and WebTransport.
- General blind CONNECT or arbitrary TCP tunneling, if a concrete product use
  case justifies the security and policy surface.
- Reverse-proxy listener products, including certificate provisioning, load
  balancing, and health checks. Existing provider seams do not imply these
  features.
- QUIC 0-RTT and connection migration only after replay and identity policy is
  explicit.

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
