# Traffic and support workflow implementation

This is the implementation checkpoint for the approved feature plan. Work is
on `codex/traffic-support-features` in the attached managed worktree. The shared
checkout contains independent updater work and is preserved.

## Completed phases

1. `20de648`: persistent privacy choices; sensitive headers collected by
   default; full request/response body retention; header presence and original
   byte sizes; non-loopback source IP display.
2. `476702d`: clipboard-only cURL and PowerShell 5.1/7 generators, quoting and
   complete-body handling, two blank lines between copied heads, lossless
   Composer loading. Body Save as appears only when clean inline generation is
   unavailable. Small binary PowerShell bodies use inline base64.
3. `7552813`: correct native verification of closed dialogs and focus
   restoration.
4. `0e27dd8`: bounded ZIP/ZIP64 SAZ parsing and compressed exports, lazy body
   reads, original headers, timing attributes and incomplete-session evidence.
5. `42d8387`: atomic saved-trace catalog imports, unique source namespaces,
   lazy native-frame and SAZ body sources, replay/inspection APIs, provenance,
   cancellation, and unavailable legacy timing/protocol/header-size fields.
6. `f6c5d50`: Traffic import UI, file activation and drag/drop, independent
   saved-capture viewers, backend window-role permissions, Composer replay,
   source trace metadata view and request navigation. Browser and real WebView2
   verification covers file launch, handoff, permission denial, independent
   catalogs, compact layout, accessibility, contrast, DPI and long labels.

## Remaining isolated phases

Current verified phase: full-header and decoded-body search, string/case/accent
and bounded regex options, quiet binary skips, complete result handles before
paging, all-match selection with reset on rerun, and whole-session selected or
unselected removal with Undo. Browser and real WebView2 checks pass; toolbar
grouping keeps dynamic status rows above the list and inspector.

- Client/proxy/upstream timing and transport observations, including incoming
  proxy connection timing; shared physical connections versus HTTP streams;
  measured L4–6 information and explicit unavailable/reused states. Persist
  these fields and import compatible SAZ timing evidence without treating local
  send completion as actual server receipt or inventing server CPU time.
- Save/export original trace metadata, with opt-in `ipconfig /all` or platform
  analogue; preserve source associations for merged traces.
- Windows captured HTML preview in a separate WebView, captured resources only,
  empty-body 404 for misses, no proxy dependency, security warning with an
  Enable scripts choice. Use an isolated untrusted-content profile and prevent
  uncaptured egress and application IPC. Other platforms may be deferred.
- Guided CLI support capture: console instructions including Ctrl+C; default
  root-install prompt without relaunch; ephemeral private key in memory only;
  default root cleanup; explicit persistent-root reuse. Keep a separate CLI
  root-ownership ledger in Local AppData, retain removal-failure metadata, retry
  orphan cleanup on later runs, and permit a fresh root without losing prior
  identities. Document start, reproduce, stop and compressed sharing.
- Graceful desktop proxy off: restore system configuration first, preserve
  active requests/connections, stop after drain, and safely cancel a pending
  off transition when switched on again. Idle clients must not block drain.
- Installer offers SAZ association. Release pipeline builds and signs a
  separate `transmog-cli.exe` artifact without bundling it in the app installer.
- Final full UX review and appropriate integrated release/interop checks.

Every substantially complete isolated phase is committed after verification.
Copy actions never execute a generated command. Additional viewers hold saved
captures only; replay is an explicit Composer action.
