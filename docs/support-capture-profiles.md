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

Import the customer trace, then open **Files → View current imports…** and choose
its original source. Incident intake shows reproduction steps, user marks,
expected/actual behavior, entered client/version, observed client/User-Agent
samples, collector version and explicit collection gaps. These sources of client
information remain separate. Selecting **Show surrounding requests** applies a
source-specific recorded-time window before virtual paging. Before/after intervals
are adjustable from 0 to 86,400 seconds. Relevant-host restriction is explicit.
Clock skew/adjustment and missing timestamps limit chronology; an empty window is
not evidence of a network failure. **Show source traffic** works without markers.

The Traffic banner exposes active time/host restrictions. **Clear incident
restriction** restores the preceding selection/scroll and keeps ordinary filters;
**Show all traffic** also clears those filters. Original inspected evidence stays
available during incident navigation. Use the existing comparison actions to
compare requests or filtered capture scopes.

**Capture tools → Record to file → Issue happened here** uses the same durable
marker model. These file-only recordings still do not populate live Traffic;
import the resulting file to inspect its incidents.

**Save trace…** reviews all retained entries, selected requests, current filters
(including incident windows), or an original source. The inventory includes sources,
hosts, recorded time range, body/stream availability and incident-marker counts.
Review freezes membership without pinning every body; new traffic is excluded.
Four reviews of up to 100,000 entries are retained per window. Missing reviewed
entries require a new review before saving. Host/source name inventories display
up to 100 names; counts cover the complete reviewed membership.

The original capture-context checkbox controls original capture-level metadata,
incident notes/markers, source notes and descriptive source paths. It does not
remove HTTP URLs, bodies, per-request fidelity or stream payloads. Sensitive-header
redaction also projects trailers and omits raw header evidence that could bypass
it. It is not complete anonymization. Selected exclusions are recorded in the new
copy. TMCap and extended SAZ retain source incident context; HAR and compatibility
SAZ report omitted incident-marker counts. Encryption is a new choice for the
exported copy; opening an encrypted source does not encrypt a copy automatically.
No password is remembered. Failed/canceled exports preserve original and existing
target files through the existing safe export transaction. Imported originals cannot
be overwritten by Save trace; choose a new filename.

True host/time retention filtering and extra browser context remain separate
future increments; advisory profile hints do not enable them.
