# ADR 0010: Read-only visualizer extensions

Status: accepted

## Context

Protocol inspection needs extensible parsers without placing extension code in
the trusted workbench or the live interception path. MAPI requires structured
fields, encoded and decoded byte sources, and related request/response context.
Installed extension count must not determine per-request work.

## Decision

- A versioned `.tmext` package contains inert metadata and one or more independent
  visualizer contributions. Exact request/response header-name allowlists and
  explicit body access declarations form the maximum host-enforced input grant.
- Discovery and native matching use metadata. Code loads only for an explicitly
  selected visible visualizer. Optional live feeding is declared by the extension,
  enabled by the user and scoped to related retained traffic in the same capture.
- Packages declare a stable JavaScript ES2022 module target, independent of the
  visualizer API. TypeScript is optional authoring tooling that emits JavaScript
  before packaging; the app never transpiles extension code. This keeps compiler
  language changes out of the runtime compatibility contract.
- JavaScript runs in an OS-isolated worker under the process boundary established
  by ADR 0008, with no direct or brokered network access. Visualization has a
  separate read-only protocol and cannot produce interception actions.
- Extensions return validated typed structures, source spans and derived-byte
  provenance. The host owns all rendering and interaction. Captured content and
  extension-supplied strings remain inert.
- Workers, subscriptions and mutable parser state belong to active instances,
  not package IDs. Deactivation removes native admission and releases the worker
  and captured data. Multiple visible contributions from one package can coexist.
- Extensions settings manages installation and enabled state and shows declared
  inputs. Input enforcement does not require access histories or a runtime
  auditing subsystem.

## Consequences

The same package and parser contracts work independently of the desktop shell.
Body access and live context require explicit retention, scope, quotas and stale
result handling. Package validation and worker isolation are complementary;
hashes identify code revisions rather than authenticate a publisher. Bundled and
development extensions use the same read-only execution boundary.

Implementation details and remaining work belong in the
[extension guide](../extensions.md) and [roadmap](../roadmap.md).
