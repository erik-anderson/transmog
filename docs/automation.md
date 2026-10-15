# Automation and auto-responses

`transmog-automation` is an optional declarative layer over the core
interception API. It is not a second execution engine and is not required by
`transmog-core`. Embedding applications may register their own
`ExchangeInterceptor` implementations instead of, or alongside, compiled
rules.

The crate compiles a finite rule set into ordinary identified registrations.
Every applied request or response edit therefore appears in the core
hook-effect audit trail under a stable identity such as
`automation/<rule-id>/<request|response>`. Rules cannot bypass interception
deadlines, cancellation, resource limits, or effect attribution.

The application layer persists validated rule generations, response assets,
and script revisions. Activation is atomic: an invalid candidate never replaces
the last working snapshot, and an in-flight exchange keeps the immutable
snapshot with which it started.

## Response experiments

Select one complete captured response in the main window, then choose **Actions →
Create experiment**. Name the hypothesis and edit its status, headers or body in
the existing response editor. **Test match** checks the request conditions without
sending traffic; **Compare with captured source** shows the unsaved difference.
Switching workspaces preserves the draft. Failed saves keep it available for retry.

**Save experiment** creates an experiment with an empty **Baseline** and an
**Edited response** variant. Each saved variant revision contains complete ordered
rules and exact response-asset revisions. Saving never changes live traffic, even
when an earlier revision is active. Unrelated rules and the global On/Paused
setting are preserved. The original **Create auto-response** flow still creates
ordinary rules with its established save behavior.

Open **Automation → Experiments** or **Named variants and revision history…**
from the shared response editor. Select an experiment, variant and revision to
inspect them without changing live participation. Duplicate a saved revision to
create another approach, rename it, include/exclude or reorder its rules, and use
**Add selected response** to add another captured response through the same editor.
Earlier revisions remain inspectable; duplicate an earlier revision to edit it.
Archive keeps identities and history, and **Restore (Undo archive)** reverses it.
Activate Baseline or another variant before archiving active content.

**Preview saved revision…** uses the exact selected definitions and included rules,
independently of live activation and pause. Individual rule participation is locked
for that run. **Preview baseline…** uses the captured scene without this experiment.
For a script or stylesheet, choose a retained HTML page from the same source.
Recorded referrer and timing are hints, not proof of which page used the resource.
If that source has expired, select a complete HTML response in Traffic first.
Browser session/reload behavior is documented in [captured-page
preview](captured-page-preview.md).

**Use saved revision for live traffic** explicitly replaces only this experiment's
live subset, in one validated persisted transaction. Possible overlaps with other
native rules are shown first; **Activate despite overlaps** acknowledges those
warnings. Conservative overlap warnings are not proof that two rules match the
same real request. A validation or persistence failure leaves the previous live
configuration intact. For an equal-priority conflict, expand **Activation order**
and save a revision before or after other live rules, then activate that revision.
This changes only the selected variant; unrelated rule priorities stay unchanged.

Only one revision participates per experiment. **Baseline** means no rules from
this experiment; unrelated automation remains active. **Pause experiment** gates
participation without rewriting authored rules, enabled choices or revisions.
Activation and resume preserve the global auto-response pause. **Undo activation**
restores the previous owned subset only if no automation generation has changed
since that activation. On conflict, current work is preserved; inspect saved
history and explicitly activate the desired revision instead.

Definitions and revision history survive restart in the existing automation
workspace. Limits are 32 experiments, 16 variants per experiment (including
Baseline), 32 revisions per variant, 64 rules per revision, and the existing 4 MiB
workspace limit. Reaching a limit rejects the save and preserves the draft;
history is never silently discarded. Display names are separate from stable IDs.
Captured source links remain on the saved assets after source Traffic is evicted.

An immutable linked-file asset reference identifies configuration, not historical
file bytes. A new preview or live request reads a fresh coherent file snapshot;
F5 retains the current preview's frozen bytes. Earlier actual results remain
inspectable under the existing Traffic body-retention policy and response file evidence.
Selecting an old configuration does not recover missing, truncated or expired
response bytes. Portable experiment packaging is a later increment.

## General rule ordering

General mutation rules are ordered by ascending numeric priority and then by
stable rule ID. Higher-priority edits are applied last on both request and
response legs. The compiler rejects equal-priority rules whose predicates can
overlap and write the same header or body. Callers must give those rules
distinct priorities or make their predicates disjoint.

The desktop presents built-in operations, such as conditional User-Agent
changes, as task-specific editors. Internal IDs, revisions, and priorities are
kept out of ordinary authoring UI while remaining available for persistence,
stale-update detection, and audit evidence.

## User-Agent and client hints

UA header overrides contains an ordered list of User-Agent rules. The first enabled
matching rule wins, even if the request has no headers to replace. Host filters
are exact and case-insensitive; path prefixes are case-sensitive. Drag the
handle or use Move up and Move down to change precedence. Saving filters already
used by another rule asks whether to replace that rule, retaining its position.
Deleting a rule offers Undo. List changes affect newly admitted requests.

The editor includes 48 presets in ten groups and accepts custom values. `%s`
and `%full%` resolve to the source browser's major version followed by `.0.0.0`;
`%major%` resolves to the major alone. An optional major or dotted version
overrides this automatic choice. Unknown source versions preserve fields that
need version substitution. Choosing the Googlebook preset resolves its Android
platform version using [Chromium's calendar heuristic](https://chromium.googlesource.com/devtools/devtools-frontend/+/HEAD/front_end/models/emulation/DeviceModeModel.ts): year minus 2011 through
September, year minus 2010 from October. The resolved value is saved with the
rule and remains editable, so an existing override does not change with the date.

Replacements affect existing User-Agent and selected Sec-CH-UA fields only,
preserving duplicate fields and never adding missing headers. Client hints can
be preserved, overridden from preset metadata or custom values, or stripped.
Preserve is the default when no hint overrides are authored. The Automation
navigation count includes enabled non-auto-response rules and active script
modules; UA header overrides shows its enabled header-rule count.

## Ordered auto-responses

Auto-response rules form a compact top-to-bottom list beside a resizable
properties pane. The first enabled rule that matches wins and bypasses the
origin. Selecting one rule opens its properties; selecting several exposes
bulk actions through More. Search and enabled/disabled/shadowed filters preserve
the underlying priority order. Drag above or below a rule to place a selected
group, preserving its internal order. Holding near the list's top or bottom
scrolls while dragging; Escape cancels and restores the prior selection.
Move earlier/Move later or Ctrl+Alt+Up/Down also changes priority. Reordering
preserves unsaved properties, and list Undo restores the previous order. New rules are inserted
first. Disabled copies provide a convenient starting point for a variant.

The primary creation flow starts from a completed traffic item. **Create
auto-response** copies the exact client-visible status, ordered response headers,
and encoded body into an immutable response asset, then pre-fills a method and
exact normalized absolute-URL match.

HTTP port 80 and HTTPS port 443 are implicit in created rules; matching accepts
either spelling. Nondefault ports remain explicit. Revert restores the captured
draft and its source. Status, content type, headers and file replacement are
available before the first save, with protected header bytes retained.
The primary Automation action opens Traffic to choose responses, or uses the
current reusable response. Multiple
selected rows open a review with eligibility,
priority and duplicate explanations in a compact table. Excluded responses stay
visible, and refresh keeps their inclusion choices. The review can skip
responses or retain only the earliest or most recent response per identical
match. Creation leases
every eligible source before copying and activates the whole batch atomically;
a failed source never leaves a partially activated batch. Creating a response
from scratch is available in the secondary More menu. Captured responses remain
the primary path because they retain protocol-relevant headers.

Captured sources require a complete retained client response. Disabled,
truncated, evicted, missing, or lossy bodies cannot be represented as faithful
sources. Text can be edited in decoded form and re-encoded with its original
gzip, Brotli, deflate, or zstd stack, or the user can deliberately choose
identity output. Complete non-Unicode bodies open in Hex mode. Text and Hex view
changes retain the same bytes; invalid text conversions are rejected without
changing the draft. Unsupported text encodings remain editable in Hex without
silently changing the declared charset.
Unchanged bodies retain their exact encoded replay bytes. Oversized sources retain
exact replay behavior and offer bounded inspection or file replacement without
treating a preview as the full editable body.

Matching uses the original client request, before request mutations. Method and
case-sensitive exact URL are the default conditions. Captured response headers,
including Vary, do not add implicit request-header or User-Agent restrictions.
One modified response can therefore serve different browsers; add a request-header
condition explicitly when a rule should target only one client. Methods include Any and
custom tokens. Request-header conditions are available for every method;
request bodies are not inspected. Exact URLs, address patterns and bounded
regular expressions use the same backend matcher in live requests and in the
network-free editor tester. Saved test cases retain their own method, URL,
explicit headers and expected result. The tester separates the request, observed
result and saved cases.
Each case shows Expected, Actual and Test (Passed or Failed); a passed negative
case means the request correctly did not match. Adding an expectation never
changes matching. Cases become durable with Save rule, and distinct header
variants remain separate. A rule can retain up to 32 cases. Generalizing a
selected path segment and editing a placeholder show the same syntax inline.

Address patterns keep the scheme and host literal. In the path, `{id}` or `{}`
matches one nonempty segment; `{id:digits}` or `{:digits}` matches decimal
digits; a final `{path...}` or `{...}` matches one or more remaining segments.
Names are optional annotations and have no effect on matching. Repeated names
do not require equal values. Literal path portions are case sensitive by
default. Query matching is explicit: exact (including absence and ordering),
required decoded name/value pairs (including repeated names), or ignore.
Regex rules explicitly select URL or path scope, whole-value or substring
matching, and case sensitivity. Expressions compile once under finite bounds.

The persisted autoresponse gate controls registrations for newly admitted
requests without changing any rule's enabled state, revision, response or
priority. Pausing leaves other native automation active. Existing exchanges
retain their original hook snapshot. New workspaces start with the gate on.
Validation and activation reject stale generations.
The same On/Paused control appears in Automation and Traffic.

Guaranteed shadowing diagnostics compare equivalent matching behavior in
first-match order, ignoring annotations and regression examples. They identify
the earlier enabled rule; arbitrary regular-expression equivalence is not
inferred from sample URLs.

Saved response status, content type, ordered headers and body bytes remain editable
after the original Traffic entry disappears. Body replacement from a file also
supports binary responses. **Serve response body from file…** opens the file
picker, then **File use** offers two explicit choices for files up to one GiB:

- **Copy into response** (default) copies bytes when the rule is saved. External
  edits do not affect that revision. **Saved copy from** retains the source path
  as context; serving never reopens that path. Copied bodies within the existing
  editor bounds can be edited inline after saving.
- **Read file on each request** retains the absolute path and rereads current
  bytes for each matching request, including small CSS/JavaScript files. Saving
  an external editor changes future live responses. Preview snapshots the file
  when opening; **Open updated preview…** reads new bytes, while F5 keeps the
  existing run's frozen responses and browser state.

File size never switches the selected mode. Changing a linked rule to Copy saves
its current file bytes in a new immutable revision. Selecting another file keeps
the pending mode; **Clear file choice** resets it to Copy for the next selection.
File mode disables inline body editing and offers **Clear file choice**.
Clearing a pending file choice preserves the text draft. Compression controls
use canonical names (`x-gzip` maps to `gzip`); captured headers and source context
retain the original coding token.
The file panel checks current availability without loading its body into the UI.
**Check file** retries the check; a last serving error remains visible until a
successful response clears it. **Choose another file…** repairs a moved or
unreadable source while retaining other rule properties. Missing links retain
their rule, mode and path across restart, fail the matched request without falling
through to the origin, and explain recovery beside the rule. Failed saves keep
the pending path and mode for correction or retry. Copies remain usable after
their original source moves or disappears.
Content-Length always reflects the final encoded
body bytes. Compressed edits rebuild Content-Encoding from the applied stack;
identity edits remove it. Apply original compression type names the original
stack and applies it to text and binary replacement files. Copied files store
encoded bytes; linked files freeze and encode current input before streaming with
bounded backpressure, without reusing a stale Content-Length. Pasted framing and encoding
fields cannot override these choices in the editor. Response edits create an
immutable configuration revision; copied bytes, earlier revisions and captured evidence remain
intact. The response body editor
starts at twelve lines and automatically selects Text or Hex for complete editable
bodies. Edit in dialog supports both representations and syntax editing for SVG,
HTML, CSS, JavaScript, JSON or plain text. SVG uses HTML language services while
retaining its content type; SVG formatting is disabled to preserve XML semantics.

Formatting for supported languages is explicit and undoable. Monaco popups remain
inside the modal dialog and can extend beyond the editor. Apply to draft updates
the draft; Cancel retains its previous body. An existing
rule with only body changes also offers Save rule. Failed saves keep the dialog
and edits available, and closing restores focus and releases the editor model. The open Response section groups status, content type,
ten-line editable headers and neutral original-response context; rule usage is
shown with the rule properties. A friendly source link opens the original request when
it is still retained; otherwise the label remains plain text. Match counts and
last-match times describe only retained Traffic.

Saved-rule enabled switches apply immediately. Property edits remain drafts
until Save (Ctrl+S); Revert resets them. Changing rule selection with unsaved
edits offers Save and continue, Discard changes or Keep editing. Traffic and
rule lists share Ctrl+click, Shift ranges, Shift+arrow, Ctrl+Shift+arrow, Ctrl+A
and Escape. Focus can move independently with Ctrl+arrow. Del removes selected
list entries, while text fields retain their normal editing shortcuts. List
actions can be undone with Undo or Ctrl+Z. Traffic selection survives scrolling and
refresh changes and includes offscreen entries; changing a filter
or search clears it. Removing Traffic entries hides them from the live catalog
without deleting capture files or saved responses. Undo succeeds while the
underlying entries remain retained.

When a rule wins, session summary and detail data retain its friendly name,
stable ID, revision, evaluation position, response asset, status, and body
size. The traffic row is marked `AUTO`, and the response inspector links to the
matching live rule. Imported traces retain their historical rule identity in
Details without mapping it onto a local rule definition. That immutable evidence
remains meaningful after a rule is
renamed, reordered, disabled, or deleted.

Scripts have their own Automation tab. **Save & enable** validates, saves and
activates a new or paused script in one action. **Save & apply** does the same for
edits to an active script. Validation is authoritative in the backend; activation
starts the sandbox and persists the exact source and settings before atomically
replacing the running revision for newly admitted requests. Failure preserves the
previous active version and the current edits. In-flight requests retain their
original snapshot. Concurrent changes invalidate stale candidates.

**Save draft** (Ctrl+S) remains available without enabling the script, including
unfinished source. **Validate only** is available in More. **Test in sandbox**
automatically validates when needed and evaluates the authored sample without
sending a request or changing traffic. Testing is optional. Its result identifies
the handler and returned action; it describes that input only. Editing source,
capabilities or test input marks earlier test results outdated.

Live state and draft state are separate. An active script can have unapplied
changes, and a paused script can have a saved draft. Successful activation makes
that exact source and settings the saved editing baseline. Reopening recovers the
active version when it is newer than the saved draft, while retaining newer saved
work. Late asynchronous results cannot mark newer edits saved or enabled. Pause
is directly available while running; More offers comparison of both source and
settings. Sandbox input can be authored or copied from a selected Traffic request
with sensitive headers omitted.

### Linked-file snapshots and historical evidence

Each live linked response acquires an owned snapshot before returning response
headers. Input is read with bounded buffers and verified by a second read/hash
and unchanged length/modification time. Windows excludes in-place writers while
the handle is open; atomic replacement is allowed, with the next request opening
the replacement. A detected change or read/encoding failure fails the matching
request without contacting the origin. Finish saving the file and retry; there
is no automatic retry loop. On other platforms the verification detects changes
but does not provide a filesystem transaction against uncooperative writers.

Compression completes against the frozen input. The response pump reads owned
bytes, so an external edit, deletion or rename during transfer cannot alter them.
Active snapshots deduplicate identical encoded content and are bounded to 128
files / 2 GiB, plus one preparation with at most 1 GiB input and 1 GiB encoded
output. These are temporary serving files, released with their consumers; limit
pressure fails the request with a retry notice. This bound is separate from
historical capture retention and does not change the existing one-GiB file limit.

In Traffic, **Details → Timings → Response file evidence** shows the captured
rule/asset revisions, selected encoded entity length/codings/SHA-256, and the
captured client-response hash and availability. Later response processing and
HTTP no-body rules can change the output; selection is not proof of client
receipt. Historical inspection and comparison use the exchange's retained
client-response bytes, never its current external file. Different file versions
under one configuration revision keep distinct identities. The existing body
retention/privacy limits still apply: incomplete, disabled and evicted evidence
cannot be reproduced by rereading a path. Complete imported bodies remain lazy;
if their aggregate hash has not been computed, Details says **Not computed** and
comparison reads their retained bytes to compute it. Save a TMCap trace while complete bytes
remain available to preserve them and their selection identity.

## Body and script safety

Body replacements operate on the declared decoded representation. Request-body
replacement is enabled by default only for methods known to be idempotent. A
rule must explicitly acknowledge modification of a non-idempotent request; the
acknowledgement records the risk but cannot make the operation intrinsically
safe.

Compilation enforces caller-selected finite limits for rule count, header
operations, regular-expression size, and replacement-body bytes. Scripts use
the same validated action vocabulary but run in the separately sandboxed script
host described by [ADR 0008](adr/0008-traffic-workspace-and-script-isolation.md).
Script errors abort the affected exchange and produce bounded remediation
diagnostics.

## Captured-page experiments

The main window's [captured-page preview](captured-page-preview.md) can snapshot
selected saved auto-responses, including disabled rules. Preview participation is
independent of live enabled states and the On/Paused gate. Preview never changes
those states. The same request matcher evaluates actual browser method, normalized
URL and explicit header conditions before captured fallback; the first
participating match wins. Browser headers may differ from captured headers and
captured Vary is not an authored rule condition.

Save editor changes or explicitly exclude unsaved changes before opening a modified
preview. **Preview captured page…** offers source-local HTML choices for captured
resources, reusing a matching retained scene when available. Authored responses
without a captured source use the last retained scene or selected complete HTML
response. Existing previews and F5 retain their
original rules and linked-file bytes; **Open updated preview…** freezes the new saved
versions. Diagnostics retain historical rule/asset revisions and byte hashes even
when the current saved rule changes or is deleted.

## Compare a response draft

The response editor offers **Compare draft**, **Compare with captured source**,
and **Compare with previous saved revision** when its source/revision is retained.
The shared comparison freezes the current status, ordered headers and proposed
body without saving or activating the rule. Invalid status remains visible as
proposed input. Header/byte editing errors stay beside the editor and preserve the
draft. If the original capture was removed, its comparison side is unavailable;
an earlier saved revision is offered only when it exists.

Saved assets and drafts are response-only subjects. Their request/outcome/timing
fields are not applicable. Copied assets preserve their encoded entity bytes,
with separate decoded/text verdicts. Draft/linked-file input is labeled as decoded
bytes. Generated framing/compression stays distinct from original captured entity
bytes; this is not proof of a downstream observed response. Current linked
file bytes are acquired coherently and labeled with acquisition time. A later file
edit cannot change an open result. **Preview captured page…** still uses explicitly
saved rule snapshots; comparing does not save a draft for preview.

See [Traffic comparison](traffic-inspection.md#comparing-captured-evidence) for
representation, coverage, exclusion and processing limits.
