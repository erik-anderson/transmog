# Product state and support diagnostics

Transmog keeps product preferences above the proxy/session layers. The
`transmog-app` facade owns validation and persistence so the Windows desktop
and a future command-line frontend share the same privacy and failure
semantics.

## Persisted state

The current schema is version 2 and contains only:

- theme, bounded session page size, and the default system-proxy choice;
- explicit body-retention, recent-artifact, and support-bundle path choices;
- bounded window geometry; and
- at most 20 recent artifact references when the user opts in.

Live sessions, captured bodies, request/response headers, credentials,
breakpoint envelopes, controller capabilities, replay drafts, and CA private
keys are not product settings.

Each save writes and flushes a create-new temporary file, then atomically
renames it to a monotonically numbered generation. The newest valid generation
is authoritative and at most three valid generations are retained. This avoids
Windows' non-replacing `rename` behavior without creating a corrupt window
between removing and replacing one canonical file. Startup validates the
256-KiB file bound and every field before use. A bad newest generation is
quarantined and an older valid generation is used; if none is valid, safe
defaults are loaded. Only the explicitly bounded v1-to-v2 migration is
supported.

On Windows these files live under `%LOCALAPPDATA%\Transmog` using the
`preferences.<generation>.json` name family. Save, migration, read-only
directory, and storage failures are reported as bounded diagnostics and never
participate in proxy shutdown or host restoration.

## Operational diagnostics

Operational events have a timestamp, severity, stable component and code, and
a bounded redacted message. Memory retains at most 256 events. The optional
JSON-lines log rotates at 1 MiB and failures to create, append, flush, or rotate
it are non-fatal. Messages containing credential-bearing header names,
cookies, passwords, bearer credentials, private-key markers, or local paths
are replaced before either sink sees them.

The Windows desktop always configures this sink as
`%LOCALAPPDATA%\Transmog\diagnostics.jsonl` and writes a startup event as soon
as the application facade initializes. Command dispatch, proxy lifecycle,
Windows host integration, certificate create/trust/remove outcomes,
live-session subscription, and bounded frontend exceptions are recorded
there. A prior `diagnostics.jsonl` rotates to
`diagnostics.jsonl.1`; neither file contains captured traffic. Settings →
Support → Technical details displays the exact active path so support reports
do not depend on knowing the Tauri package identifier.

The diagnostics report includes the Transmog version, selected dependency
versions, OS/architecture, the native WebView runtime version, the state schema,
and the bounded event list. It contains no captured traffic.

Support bundles are create-new ZIP files with only `diagnostics.json` and a
validated `product-state-summary.json`. They exclude bodies, HTTP header
values, credentials, and private keys unconditionally. Recent paths require
both a persisted privacy opt-in and a per-export opt-in. Failed generation
removes only the newly created partial bundle and never replaces an existing
destination.

## Recovery and support procedure

1. Stop the proxy normally; state/log failures cannot block this step.
2. Use **Settings → Support → Refresh diagnostics** to inspect the
   readable summary and copyable technical report.
3. Choose a new destination and create a support bundle. Leave path inclusion
   disabled unless the paths themselves are needed to diagnose a problem.
4. If preferences are corrupt, restart Transmog. It automatically quarantines
   the bad generation and reports whether it recovered an older generation or
   loaded defaults.

Product-state generations, operational logs, and support bundles are not
capture databases. Native `.tmcap` remains the durable traffic format.
