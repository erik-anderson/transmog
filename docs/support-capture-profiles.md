# Support capture profiles and incidents

A support capture profile describes a website investigation. It is distinct from
Transmog's network profiles and from operational support bundles for Transmog
itself. Profiles contain plain data; they cannot authorize certificate trust,
execute commands, select output/password files, launch a browser, or upload data.

Start with the [example profile](examples/support-capture-profile.json), replace
the example hosts and reproduction steps, then validate it with the distributed
CLI. No source checkout or Cargo installation is needed:

```powershell
.\transmog-cli.exe profile validate --input .\issue.json --format json
.\transmog-cli.exe record --profile .\issue.json --output .\customer.tmcap --encrypt
```

Validation reads at most 64 KiB, rejects unsupported versions/unknown fields, and
has no certificate, routing, credential or output side effects. JSON validation
results contain `schemaVersion: 1`, `valid`, `profileId`, `revision`, and a SHA-256
`digest`. The digest identifies the validated, normalized profile content; it is
not a signature or authorization token. Invalid input exits with the existing CLI
error status and a field or JSON location in the diagnostic.

## Profile version 1

`schemaVersion`, `id`, positive `revision`, and `title` are required. IDs use
1–64 ASCII letters, numbers, dots, dashes or underscores. Titles allow 160 UTF-8
bytes, support references 160, instructions 8,192, and expected-application text
256. Up to 32 ordered steps allow 1,024 bytes each. Up to eight incident prompts
allow 256 bytes each. Hidden terminal controls and directional override characters
are rejected; authored text is displayed inertly.

Up to 64 `relevantHosts` contain an exact hostname and optional
`includeSubdomains`. IDNs normalize to ASCII for matching; host matching is
case-insensitive and requires a label boundary for subdomains. URLs, ports,
credentials, wildcard/regular-expression scope and paths are rejected. IPv6
addresses use brackets. Hosts are **advisory health and intake hints**. They do
not restrict interception or retention; other hosts can be saved.

`recording` optionally suggests `redactSensitiveHeaders`, `requestBodyLimit`
(`{"kind":"limited","bytes":25000000}` or `{"kind":"unlimited"}`),
`circularBuffer` (`auto`, `unlimited`, or the existing byte/MB/GB/MiB/GiB syntax),
and `includeNetworkContext`. Encryption is `unspecified`, `recommended` or
`required`. Recommended encryption is visible advice; `--encrypt` selects it.
Required encryption enables the existing local password flow and cannot silently
fall back to plaintext. The profile never contains passwords or password paths.
A persistent-root suggestion is advice; only explicit `--persistent-root` selects
persistent trust, with the existing OS consent and ownership lifecycle.

Explicit command-line choices win, followed by profile choices, stored CLI
preferences, and built-in defaults. Only existing explicit privacy/body-limit
flags update those preferences. Profile-only choices are run-scoped. Preflight
shows resolved settings/origins, output, encryption, optional network context,
reproduction steps and advisory scope. Original capture context retains the
validated profile, digest and resolved settings without credential paths.
`--client-version`, `--expected` and `--actual` add bounded user-entered context;
they do not establish which browser actually produced recorded requests.

An optional `durationSeconds` (1–86,400) requests normal stop/drain; explicit
`--duration SECONDS` overrides it. Omission means manual stop. The existing first
Ctrl+C restores routing before draining; a second stops unfinished work. An
explicit duration follows the same path and reports why recording stopped.

## Recording and incident durability

After setup, an interactive console accepts `m` then Enter for **Issue happened
here**, or `m short note`. Notes allow at most 2,048 UTF-8 bytes; there are at
most 256 markers per recording. Input ownership starts after all setup/password
prompts and ends before drain/cleanup. Redirected/noninteractive stdin disables
marking. Closed or unavailable incident input leaves Ctrl+C available. No global
keyboard hook is installed. On Windows, an owned inbox PowerShell console reader
is terminated and joined before input ownership returns to other prompts.

Collection health reports aggregate exchanges, HTTP/relevant-host observations,
decrypted HTTPS, CONNECT tunnels and failures, without request URLs or header
values. No relevant traffic and tunnel-only evidence are distinct from a website
failure. A valid sealed file does not prove that the reported issue was captured.
Save, retained evidence, routing restoration and certificate cleanup are separate
outcomes; retain any reported recovery file on failure.

A native `incident-marker` is an artifact-level extension record with sequence
and exchange ID zero. Its version 1 payload contains independent random `id` and
`runId`, UTC `unixMillis`, optional monotonic `offsetMillis`, `origin`
(`cli`, `desktop`, `analyst`), `creationKind` (`user-mark`, `later-annotation`),
and plain `note`. It is never a fake HTTP exchange or a request-order number.
The initial UI creates user marks; later annotations remain distinguishable in
the schema. The writer serializes marks with capture commands and acknowledges a
normal-file marker after checkpoint publication. Stop/seal races either retain an
acknowledged mark or return an explicit failure; they never write after seal.

Markers receive native compression/encryption and survive native import,
source-preserving save/merge, valid-prefix recovery, and supported circular
export. Circular exchange eviction does not discard incident metadata, but can
remove surrounding requests. Memory circular buffers remain unsaved until export
and are lost on abrupt process termination. Disk circular scratch storage has the
existing recovery limitations. Checkpoints publish through the OS without fsync;
they do not promise power-loss durability. See [native capture](native-capture-format.md).

## Developer intake and export review

Opening a customer trace exposes **Markers** above Traffic. The panel expands
in a fresh capture viewer when markers are present. It uses a compact marker
list beside the selected note and actions, leaving the request list and inspector
available. Choose a trace when several original sources are loaded. Reproduction
steps, expected/actual behavior, client evidence and collection gaps live under
**Capture details**; raw context remains in **Files → View current imports…**.

Timeline shows flags in its overview and visible range. Click a flag or marker
row to go to its recorded time and scroll the nearest matching request into view.
Overlapping flags open a chooser. Keyboard users can Tab to a flag, use Left/Right
to move between flags, and Enter to open one. Navigation preserves request
selection and ordinary filters. **Show nearby requests** explicitly applies a
source-specific time filter, initially 30 seconds before and after the marker.
**Time range** adjusts those intervals from 0 to 86,400 seconds and optionally
restricts suggested hosts. **Clear time filter** restores the preceding view;
**Clear other filters** also removes ordinary Traffic filters. **Show full trace**
works when no markers are present. Source clocks may differ, and missing nearby
requests do not establish a network failure.

Use **Flag request…** or **Flag response…** in a request's context menu or the
inspector's **Actions** menu to add a reviewer marker with an optional note.
These markers retain an exact message link through supported copies, independently
of workspace request numbers. A response marker uses recorded response-header
timing when available; otherwise it explicitly uses the request time. Requests
without recorded timestamps cannot be flagged. Selecting a linked marker also
opens its request or response in the inspector. A flag beside the request number
identifies flagged requests in List.

**Edit note → Apply** changes the review copy. **Clear note** removes only its
text; **Delete marker** removes its timestamp, note and message link. **Undo**
restores recent marker changes. **Edited note** identifies revised text; **Unsaved
changes** means the current markers have not been preserved in a supported copy.
Drafts survive trace and view changes. **Save copy…** includes pending note edits;
closing a window with unsaved marker work offers saving, discarding or continuing
review. Clearing traffic keeps its capture-level markers available, including
when no requests remain.

**Capture tools → Record to file → Issue happened here** uses the same durable
marker model. These file-only recordings still do not populate live Traffic;
import the resulting file to inspect its incidents.

**Save copy…** reviews all retained entries, selected requests, current filters
(including incident windows), or an original source. The inventory includes sources,
hosts, recorded time range, body/stream availability and incident-marker counts.
Review freezes membership without pinning every body; new traffic is excluded.
Four reviews of up to 100,000 entries are retained per window. Missing reviewed
entries require a new review before saving. Host/source name inventories display
up to 100 names; counts cover the complete reviewed membership.

**Include markers and notes** controls the current marker set independently of
**Include capture details**, which controls recorded configuration, reproduction
context, source notes and descriptive paths. TMCap and extended SAZ retain marker
text, reviewer provenance and exact message links. HAR and compatibility SAZ
omit them and disclose the omission before saving. Exported copies contain
current notes only, without replaced text, deleted markers or session Undo history.
All remaining markers for included sources are preserved even when only some
requests are selected. A link to a request omitted from the copy stays unavailable.

Export review checks marker revisions; edits made after review require a refreshed
inventory. Edits made during writing remain unsaved. Failed, canceled, or
marker-omitting exports keep unsaved state. Imported files cannot be overwritten;
choose a new filename. Encryption is a separate choice for each copy, with no
remembered password. Header removal is not complete anonymization and does not
remove private data from URLs, bodies or notes.

True host/time retention filtering and extra browser context remain separate
future increments; advisory profile hints do not enable them.
