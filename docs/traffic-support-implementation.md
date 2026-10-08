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
7. `42dd7e1`: full retained-header and decoded-body search, quiet binary skips,
   bounded regex, all-match selection and whole-workspace removal with Undo.
8. `be92a19`: graceful desktop Off restores host settings first, keeps admitted work alive,
   excludes idle clients, and permits resume on the same listener. Generations
   invalidate old shutdowns and stale UI status. Real streaming integration,
   protocol matrix, browser and native WebView2 checks pass.
9. `ee3b7f8`: guided `transmog-cli record`, console Ctrl+C instructions, memory-only
   ephemeral keys, a separately protected multi-root ownership ledger and
   recovery, explicit persistent reuse, persistent redaction preferences,
   force stop with cleanup, and streaming compressed native captures. Bounded,
   cancellable gzip imports keep body reads lazy. CLI, facade and host tests,
   browser checks, a real console capture, and native compressed-viewer checks
   pass. The support guide covers start, reproduce, stop, review and sharing.
10. Optional current-user SAZ registration, owned-path uninstall and update
    preservation. Standalone CLI release build/signing, four-part Windows
    resources, required release asset/checksums/signature evidence, qualification
    and provenance. CLI tests, compiled NSIS bundle/isolated registry fixtures,
    script parsing and release-provenance/publication/lifecycle checks pass.
    Actual signing remains part of the protected release workflow.

## Remaining isolated phases

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
- Final full UX review and appropriate integrated release/interop checks.

Every substantially complete isolated phase is committed after verification.
Copy actions never execute a generated command. Additional viewers hold saved
captures only; replay is an explicit Composer action.
