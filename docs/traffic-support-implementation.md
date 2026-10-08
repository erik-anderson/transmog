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
10. `329c175`: optional current-user SAZ registration, owned-path uninstall and update
    preservation. Standalone CLI release build/signing, four-part Windows
    resources, required release asset/checksums/signature evidence, qualification
    and provenance. CLI tests, compiled NSIS bundle/isolated registry fixtures,
    script parsing and release-provenance/publication/lifecycle checks pass.
    Actual signing remains part of the protected release workflow.

11. `7316847`: measured client/proxy/upstream timeline and physical transport evidence,
    including client acceptance, first read, process attribution and TLS; Hyper
    DNS/TCP/TLS setup and failures, exact HTTP boundary versions/reason phrases;
    shared pool counters and measured QUIC path/recovery statistics. Native v3
    persists bounded evidence and keeps v1/v2 readable. Traffic Timings provides
    overlapping phases, UTC timeline, reused connections and original SAZ timers.
    Monotonic/merge/I/O/pool/import tests, protocol matrix, desktop tests, clippy,
    browser UX and native WebView2 checks pass. Reviewed populated and compact
    layouts; improved legacy import presentation to put saved timers first.

12. `be7193c`: source-aware streaming native/gzip Save trace in main and saved viewers, with
    original source metadata, request identifiers, heads/timers and terminal times.
    Optional bounded network configuration in desktop recording/saving and CLI,
    with platform, UTC collection time and failures shown in Trace metadata.
    Original context survives SAZ conversion. Aggregate merged import limits are
    enforced before atomic publication. Round-trip, missing-body, removal, failed
    overwrite, metadata quota and Unicode subprocess tests pass. Browser and native
    UX checks cover defaults, cancellation, retry drafts, compact/populated layout.

13. Windows captured-page preview: separate fresh-profile WebView2 windows,
    bounded frozen responses from the selected original trace, nearest-time
    method/URL matching, empty 404 misses and no proxy dependency. The warning
    defaults scripts off and offers opt-in scripts per window. Native all-source
    resource interception, deny-only transport fallback, app IPC denial and
    renderer restrictions prevent uncaptured traffic. Preparation cancels, failed
    hidden windows close, and owned response files live with their consumers.
    Scene, browser and real WebView2 checks cover source isolation, resources,
    script choices, misses, uncaptured loopback egress and window closure. Reviewed
    compact and populated warning layouts. See docs/captured-page-preview.md.

## Remaining isolated phase
- Final full UX review and appropriate integrated release/interop checks.

Every substantially complete isolated phase is committed after verification.
Copy actions never execute a generated command. Additional viewers hold saved
captures only; replay is an explicit Composer action.
