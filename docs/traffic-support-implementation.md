# Traffic and support workflow implementation

This is the completed implementation record for the approved feature plan. Work is
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

13. `7fc8e95`: Windows captured-page preview: separate fresh-profile WebView2 windows,
    bounded frozen responses from the selected original trace, nearest-time
    method/URL matching, empty 404 misses and no proxy dependency. The warning
    defaults scripts off and offers opt-in scripts per window. Native all-source
    resource interception, deny-only transport fallback, app IPC denial and
    renderer restrictions prevent uncaptured traffic. Preparation cancels, failed
    hidden windows close, and owned response files live with their consumers.
    Scene, browser and real WebView2 checks cover source isolation, resources,
    script choices, misses, uncaptured loopback egress and window closure. Reviewed
    compact and populated warning layouts. See docs/captured-page-preview.md.

14. Final workflow review: copied commands preserve recorded HTTP versions and
    scope proxy credentials correctly. PowerShell explains its proxy-authentication
    limit. Compressed import cancellation also interrupts runs of empty gzip
    members. Preview profiles retry cleanup after WebView2 releases locks. Native
    probes close actual desktop windows, verify profile removal, and reopen the
    main window while preserving its catalog and independent saved viewers.
    The interoperability harness supports shared Cargo output and explicitly
    reusing installed browser dependencies in a managed worktree.

## Verification of phases 1–14

- `npm run check` and `npm run test:workspaces` pass with production templates,
  CSP and Trusted Types. Reviewed populated and compact views, keyboard/focus,
  loading, cancellation and failure/retry flows, direct clipboard commands,
  header diagnostics, search/selection/removal, metadata and save dialogs.
- Strict `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `cargo test --workspace` passes 425 tests with no failures. Its one ignored
  Docker-backed product test passes separately in the interoperability run.
- Real Windows WebView2 checks pass for accessibility, forced colors, DPI,
  long labels, layout, proxy drain feedback and single-instance behavior.
  Saved-file checks cover SAZ and compressed CLI imports, file handoff choices,
  isolated catalogs and permissions, timing/source navigation, captured-page
  resources and script choice, empty misses, no uncaptured HTTP contacts,
  actual window closure/profile cleanup and main-window reopening.
- Docker interoperability passes the full product workflow/restart test and all
  eight Chromium/cURL origin, HTTP/2, HTTP/3, content-coding and WebSocket tests.
  These controlled interop calls are separate from clipboard-only copy actions.
- Release safety/publication/lifecycle fixtures and compiled NSIS association
  fixtures pass. Standalone CLI signing and publication are required by the
  protected release pipeline; no production signed release was issued locally.

The final native cache repair rebuilt only the affected BoringSSL CMake output;
no source or shared checkout changes were needed. UI asset generation and Rust
builds run sequentially to avoid transient missing generated assets.

## Product boundaries

Captured-page preview initially supports Windows. Timings and transport metrics
report measured evidence, with unsupported points left unavailable. Native/gzip
and extended SAZ preserve merged source associations; compatibility SAZ retains
the client view with explicit normalization and evidence limits. Copy actions never execute generated commands. Saved-capture
viewers own independent catalogs; replay is an explicit Composer action.

## Audit follow-up

A comparison with the original plan identified remaining work. The user
requested fixing every gap and clarified Unlimited capture keeps the overall
storage budget. Phases 15–28 close those implementation gaps. The full feature
plan is implemented and qualified on Windows within the product boundaries above;
production signing and clean-machine release checks remain release gates.

15. Header fidelity: complete-block measurements, per-value sizes, global
    largest-first ordering and paged inspection; present/absent/unknown
    authentication states; copying the full original message block.

16. Clipboard commands: plain curl, POSIX quoting and no Windows wrapper or
    executable suffix; Invoke-WebRequest for the 5.1/7-compatible subset with
    explicit .NET fallback reasons. Removed the arbitrary 16 KiB body cutoff;
    inline generation considers the generated command length. Windows Unicode
    cURL body arguments need files because native encoding changes their bytes.
    Local raw echo qualification checks literal hostile text, credentials,
    duplicate-field fallback, binary inline PowerShell and saved body files in
    Windows PowerShell 5.1, PowerShell 7 and Bash. App copy actions never send.
    App tests/clippy, browser flow checks and documented UX semantics pass.

17. Request-body capture: 25 MB (25,000,000 bytes) default and persistent
    Unlimited in desktop settings and guided CLI recording. Per-request cap
    removal preserves the one-GiB body cache and each recording's file budget.
    Cache limits snapshot at admission; recordings snapshot their policy and
    record it in trace metadata. Native writing clips retained samples while
    preserving observed counts. Oversized observer frames split into bounded
    native records; the writer reserves room in its record budget for sealing.
    Capture limits do not reject forwarding;
    non-streaming processing keeps a separate bounded allocation policy.
    Auto streams large/unknown-length bodies through HTTP/1 or HTTP/2 instead
    of buffered H3 retries; explicitly selected H3 continues streaming.
    Known compressed entity-body wire counts are exposed with framing/TLS
    exclusions, independently of retained size. A 32-MiB real request streams
    beyond the capture/buffering limits. App, native writer, session, CLI and
    full runtime protocol/content tests pass; clippy and browser/native checks
    pass. Reviewed populated wide and compact retention settings.

18. Shared setup timing: DNS/TCP/TLS and QUIC phases carry original monotonic
    start/end offsets on each request's clock plus its actual upstream wait.
    Completed phases reused by a later exchange report zero; a request joining
    setup in progress reports only the overlap after admission, alongside the
    full phase duration and setup age. HTTP pool and H3 projections preserve
    physical identities and counters. Native serialization keeps these fields;
    legacy measurements remain explicitly unavailable for age/wait calculations.
    Core/HTTP/H3/runtime suites, protocol/content matrix, clippy, browser and
    saved-viewer WebView2 checks pass. Reviewed expanded connection facts at wide
    and compact sizes. A separate curl follow-up respects the Windows native
    command line length even when invoked through Bash.

19. Proxy work and waterfall: named request/response hook and script callbacks,
    transforms, content decoding/encoding, breakpoint decisions and concurrency
    permits are measured on the request clock. Repeated call duration preserves
    nanosecond contributions and excludes gaps, with bounded groups and explicit
    overflow accounting. Canceled/failed operations still record their duration.
    Forwarding-channel waits distinguish actual pending sends from zero waits;
    cancellation and first-body adapter/queue observations are covered. Certificate
    preparation joins the physical client connection evidence. The request
    waterfall and copyable report expose overlap, call totals, original timers,
    source IDs and delivery limitations. Core/content/HTTP/H3/runtime/session/native
    suites, clippy and browser checks pass. A real native timing capture exercises
    import/persistence and populated SVG geometry in WebView2. Reviewed wide,
    compact and native snapshots, copy feedback and reachable dialog actions.

20. Read-only Windows SIO_TCP_INFO and Linux TCP_INFO sampling records kernel
    RTT, supported retransmission/window counters, and actual snapshot time.
    Borrowed socket handles stay in a narrow native boundary; unsupported
    platforms omit the metrics. Physical socket write/flush pending windows,
    last successful local write/flush, and byte totals are explicitly shared
    across HTTP streams and never assert remote delivery. Native captures and
    timing reports preserve the evidence. Loopback native queries, pending I/O,
    Rust transport/capture tests, strict Clippy and populated browser/native
    wide/compact timing views qualify this phase.

21. Composer replays larger complete captured bodies and replacement files
    through a bounded file stream. Loading a large or size-deferred body never
    fills the editor; switching modes preserves its draft and requires explicit
    replacement of missing bodies. Source trace/entry associations persist in
    results and bounded replay history after edits. Stream-capable executors
    opt in through execute_file; older adapters explicitly reject rather than
    sending empty bytes. Risk validation precedes captured-file staging. Native
    file selection belongs to its initiating window. PowerShell's .NET file
    fallback also streams instead of loading a whole file. Tests cover exact
    eight-MiB binary HTTP upload, native import/spooling, original associations,
    unavailable adapters, clipboard commands under PowerShell 5.1/7, body mode
    transitions and wide/compact and native viewer UX.

22. Search adds independent request/response header and decoded body scopes.
    Saved queries support bounded original-text occurrence views, including
    header identity, Unicode-normalized offsets, accented highlights and
    previous/next occurrence or entry navigation. Zero-width regexes stay
    visible. Locating normalization uses bounded range storage instead of a
    per-character body map. Search commits new results and selection only after
    both result queries succeed; canceled, failed or stale operations preserve
    previous selections. Binary bodies remain quietly skipped. Tests cover
    compressed/UTF-16 text, request scope, off-page results, composed/decomposed
    accents, surrogate pairs, case expansions, zero-width regexes and clipping,
    browser keyboard/focus and compact layouts, and saved-viewer native match
    import/API/highlighting with the proxy stopped.

23. Captured-page preview defaults to its original trace/current capture and
    explicitly offers all traffic loaded in its owner window. Frozen variants
    match exact request-body hashes and response Vary fields; selected HTML is
    fixed for initial GET navigation. Accept-Encoding is ignored after decoding.
    Bounded owner-only reports show preparation/version decisions, source links,
    browser hit/miss counts and script choice, and remain available after close.
    Header values and body bytes never enter diagnostics. WebView2 peeks or
    consumes only intercepted request streams; every path supplies a synthetic
    response or stops navigation, with a separate deny-only network fallback.
    Request bodies are bounded to 32 MiB for preview matching, with 256 MiB total
    signature and decoded-resource budgets and at most 2,048 response variants.
    These preview limits do not change capture or Composer limits. Portable
    scene tests cover body/header variants, source scope, wildcard omissions,
    original-document priority and bounded counters. Native tests cover matching
    and mismatched POST variants, CSS/images, disabled/enabled scripts, report
    ownership, compact diagnostics and continued IPC/network denial. Browser
    layout and keyboard/focus reviews cover source choices and diagnostics.

24. Export-only redaction removes sensitive header values and preserved raw header
    text from native, gzip, JSONL and SAZ copies while preserving source files and
    retained evidence. Presence and original sizes remain in native/extended
    evidence. Network context records collector/version, machine, collection
    purpose and time; the metadata view distinguishes original and save-time
    context without inventing old provenance. Round-trip/source invariance tests,
    strict Clippy, browser interactions and real saved-viewer checks pass.
    Reviewed populated native and compact save dialogs, retry and default choices.

25. SAZ retains validated trailers as structured evidence through native saves
    and extended conversions. Chunked imports stream to a sink for exact entity
    lengths/trailers; exports correctly reconstruct chunk framing. Conventional
    timers preserve original attributes and measured local observations with
    explicit delivery semantics. Actual protocols/custom reasons survive
    normalization; extended members preserve performance, redacted presence and
    sizes, IDs and merged source associations. Metadata/index/member budgets
    remain finite. All 114 app and 16 SAZ tests pass, including real file
    conversions, redaction, lazy binary reads and CRC checks; strict Clippy passes.

26. CLI roots use a backwards-readable, atomic public lifecycle ledger with
    run/validity/install/finish/cleanup timestamps, trust-store scope and key mode.
    Persistent roots nearing expiry or missing/mismatched material are retired
    and rotated; old keys are removed while exact public identities survive
    canceled OS cleanup. Proxy crash recovery runs before certificate prompts,
    including explicit roots cleanup. Older pending roots retry at completion
    without repeating the current root’s canceled prompt. Public certificate
    context joins capture metadata. Twelve CLI tests and strict Clippy pass.
    Real hidden-console tests verify Ctrl+C, sealed gzip, memory-only ephemeral
    keys, persistent reuse, forced interruption and native prefix recovery, with
    isolated profiles and loopback traffic. The guide includes signature/publisher
    checks and ready-to-copy recovery commands.

27. Final clarity review adds exact byte counts to header/total size tooltips,
    recognizes QUIC setup even when statistics are absent, and marks unrecorded
    copied protocols/reasons without inventing evidence. Mapped loopback imports
    retain local identity. The legacy serve path also forwards large requests
    independently of capture limits. All 454 workspace Rust tests and strict
    Clippy pass, plus renderer/default contracts, full browser interactions and
    native main/saved-viewer flows. Reviewed populated/compact header and timing
    screenshots. Release safety, publication, version lifecycle and compiled
    isolated SAZ registration fixtures pass.

28. Final import review charges structured trailers to the aggregate metadata
    budget, caps repeated chunk-framing issues, and bounds source notes to the
    native round-trip contract. Warning-heavy SAZ imports save and reopen all
    entries, with explicit note-omission counts. The 116 app tests and strict
    workspace Clippy pass.

## Final qualification

- The final workspace passes **456 Rust tests** with no failures and strict
  workspace Clippy. Its Docker-backed product test passes separately, along with
  all eight Chromium/cURL HTTP/1, HTTP/2, HTTP/3, coding and WebSocket checks.
- Production-template browser checks pass with CSP/Trusted Types, asynchronous
  cancellation/retry, original-text search navigation, streamed replay, export
  privacy and provenance, exact header measurements and QUIC labeling.
- Real Windows main/saved-viewer checks pass, including accessibility, forced
  colors, DPI, long labels, proxy drain/resume, file activation, permissions,
  source metadata, streamed binary replay, captured-page variants, script choice,
  empty misses, IPC/network denial and owned-window/profile cleanup. Reviewed
  populated, wide and compact screenshots for the new workflows.
- Real CLI console checks pass for Ctrl+C, sealed gzip output, sensitive-header
  defaults, memory-only ephemeral keys, persistent reuse, forced interruption
  and recovery. Root lifecycle unit tests cover canceled cleanup, expiry rotation
  and legacy migration. The support guide includes publisher/signature checks
  and copyable interrupted-capture recovery steps.
- Release payload/publication/version lifecycle and compiled isolated installer
  association fixtures pass. Protected signed-release qualification now runs the
  CLI console capture/recovery test against the signed standalone executable,
  with isolated profiles, loopback traffic and no certificate/proxy mutation.
  The installer continues to exclude that executable.

No implementation gaps remain from this audit. Captured-page preview remains
Windows-only as approved. Search and preview retain their documented processing
bounds; Unlimited request capture preserves overall storage budgets. Actual
production Authenticode signing/timestamp qualification and the maintainer's
clean Windows 11 release checklist require the protected release environment.
Signing endpoint/profile/publisher configuration is absent locally; no signed
release or publication was claimed or performed. Linux kernel-statistics code
is implemented but was not qualified on a Linux host in this Windows run.

The primary checkout's independent updater changes remain preserved. All feature
work is committed on the attached managed worktree branch.

## Additional storage and encryption work (in progress)

29. Configurable memory-first circular buffer; Automatic is half installed RAM.
30. Independently compressed and authenticated encrypted TMCap chunks with lazy reads.
31. Password-protected SAZ import and opt-in AES-256 export.
32. Desktop password flows and streaming/circular CLI recording; populated UX and end-to-end qualification.

Ordinary CLI recording streams every entry. Circular CLI recording saves only newest retained traffic at stop.

Phase 29 completed: persistent Automatic / custom GiB / unlimited disk policies, memory chunk leases and eviction, no implicit desktop journal, Settings status and populated native focus/reachability review. Validation: 133 Rust application/renderer/desktop tests, strict application/desktop Clippy, production browser workspace flow, native WebView2 and inspected Settings screenshot.

Phase 30 completed: v4 compressed frames, optional AES-256-GCM with Argon2id and agile header, authenticated indexed reads, transient wiped passwords/keys, no plaintext expansion of encrypted native files. New recordings and workspace saves use frame compression. Validation: 166 Rust capture/session/application tests and strict Clippy; encrypted native save/reopen and failed-password atomic publication verified.

Phase 31 completed: password-aware ZIP import (ZipCrypto and WinZip AES variants), opt-in AES-256 file-member export, typed password errors and encrypted native source export. Independent native 7-Zip authenticates/decrypts both export profiles and matches every member; Transmog imports 7-Zip ZipCrypto binary data. The originally requested .NET test was replaced at the user’s direction; no .NET test dependency is shipped. Validation: SAZ/application regression tests, encrypted application save/import, strict Clippy and scripts/test-saz-interop.ps1.

Phase 32 completed: shared masked transient password dialog, confirmed encryption choices for Save trace / recording / native and SAZ exports, typed password retry and Cancel for imports and inspection, chunk-compressed default saves with optional outer gzip. Production browser flow checks and real WebView2 wide/compact masking, confirmation, cancellation, focus and screenshot review passed. Native review found and fixed dynamic input type masking.

Phase 33 completed: CLI streaming records compress/encrypt on arrival; optional exchange-group circular retention uses half installed RAM automatically, custom byte/unit limits and unlimited disk. Memory circular creates no trace/cache file before stop; disk circular stores compressed ciphertext when encrypted. Passwords use masked console input or an explicitly protected file, never argv text or preferences; native recovery preserves encryption. Default CLI output is now chunk-compressed .tmcap, with legacy gzip still supported. Validation: capture/CLI tests and strict Clippy; owned hidden-console Ctrl+C tests verify encrypted streaming, memory/disk circular output, persistent root reuse and crash recovery without changing real host proxy or trust.
