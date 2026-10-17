# Automation and traffic rules

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

## Rules workspace

**Automation** has three tools: **Rules**, **Experiments**, and **Scripts**.
Rules brings saved responses, header changes, browser-header presets and response
text replacements into one searchable list and properties pane. Filter by Requests
or Responses and by enabled state; a saved response belongs to Responses because
it supplies the response before the server is contacted. Each row identifies the
action, request target and enabled state. Selecting another type preserves its
editor's draft; replacing a draft within the same editor requires resolving its
unsaved changes. Compact windows switch between the list and properties with
**Back to rules**. New rule creation stays available above both views.

**New rule…** offers **Change request headers…**, **Change browser headers…**,
**Change response headers…**, **Find and replace response text…**, **Serve a
captured response…**, and **Create saved response from scratch…**. Traffic's
**Actions → Create rule** menu prefills the selected request and action. These
operations change future matching traffic; they do not edit historical captures.
Header, text and browser-header drafts start disabled. **Save rule** retains the
authored Enabled choice; **Save and enable** explicitly enables a saved rule.
Saved-response creation retains its visible Enabled choice and existing batch
creation behavior.

The shared list supports Ctrl+click, Shift ranges, keyboard selection and batch
enable, disable, remove and disabled copies. Undo checks that the affected saved
configuration is still current. The priority column and dragging apply to saved
responses; **Execution order** disclosures expose each other editor's own ordering
and conflict controls. A shared list does not make first-match policies and
composing edits interchangeable.

**Saved responses · On / Paused** gates only saved responses, including those
used by live experiments. Enabled rules affected by this switch show **Enabled ·
Paused**. Header and text edits, browser-header rules and scripts keep their own
state. Experiments manage saved-response alternatives and revisions; Scripts is
the advanced editor for custom logic. Neither changes activation just by opening
or selecting content.

## Choose an editing workflow

| Task | Start here | Preview and activation |
| --- | --- | --- |
| Replace a captured response, compare alternatives or edit a local file | **Actions → Create experiment**, then the response editor and **Experiments** | Saved response assets participate in captured-page previews. Saving a named revision is passive; live activation is explicit. |
| Change request/response headers while retaining fresh traffic | **Automation → Rules → New rule… → Change request headers… / Change response headers…** | Standalone live rules. Test the draft without traffic; explicitly enable it for new requests. |
| Find and replace text in server responses | **Actions → Create rule → Find and replace response text…** or the same action under **Automation → Rules → New rule…** | Standalone live rules with bounded UTF-8 input. Test the draft, then explicitly enable it. |

Header rules, fresh-response edits and scripts are not members of the named
response-asset variants and do not execute in captured-page previews. To compare
policy headers in a preview, edit the saved response asset's headers. Activating
an experiment's **Baseline** removes only that experiment's live rules; unrelated
header rules, response edits, scripts and other experiments keep their own state.

### Try a captured script change

1. Select a complete captured JavaScript response in Traffic and choose **Actions
   → Create experiment**. Name the hypothesis and edit the body. The script stays
   inert in the workbench. **Test match** checks targeting; **Compare with captured
   source** compares the proposed response without saving or sending traffic.
2. Choose **Save experiment**. This saves an **Edited response** revision and an
   empty **Baseline** without changing live traffic. Failed saves preserve the
   draft. Saving further revisions also leaves an active revision unchanged.
3. Choose **Preview captured page…**, select a retained HTML page from the same
   source, review the warning and explicitly allow scripts if execution is needed.
   The page uses the saved script plus captured CSS, images and API responses.
   Missing dependencies fail as network requests; they do not contact the origin.
4. Inspect **Last preview diagnostics** for the exact variant, winning rule,
   captured source and supplied-byte hash. **Compare response** compares a retained
   supplied response; **Set response baseline** lets you compare two preview
   observations. This evidence does not establish browser acceptance or delivery.
5. Save the next edit, then choose **Open updated preview…**. It starts a new
   frozen snapshot and fresh InPrivate browser session. F5 in the earlier window
   keeps that run's saved bytes and browser state, including its cache policy.
6. Choose **Use saved revision for live traffic** only when ready. Review any
   overlap warnings. To remove this experiment's live edits, activate its
   **Baseline**; **Undo activation** is available until another automation change.
   The global saved-response pause remains independent of all these actions.

For external CSS/JavaScript editing, choose **Read file on each request** instead
of Copy. A future live request or newly opened preview acquires current file
bytes; an existing preview and historical Traffic comparison keep their frozen
evidence. See [file choices](#saved-responses), [snapshot
evidence](#linked-file-snapshots-and-historical-evidence) and [preview
isolation](captured-page-preview.md#isolation-and-implementation).

## Response experiments

Select one complete captured response in the main window, then choose **Actions →
Create experiment**. Name the hypothesis and edit its status, headers or body in
the shared response editor within **Experiments**. **Test match** checks the request
conditions without sending traffic; **Compare with captured source** shows the
unsaved difference. Switching workspaces preserves the draft. Failed saves keep
it available for retry.

**Save experiment** creates an experiment with an empty **Baseline** and an
**Edited response** variant, then returns to **Experiments** with that saved revision
selected. Editing a saved response stays in **Experiments**, and saving returns to
its updated revision. Each saved variant revision contains complete ordered
rules and exact response-asset revisions. Saving never changes live traffic, even
when an earlier revision is active. Unrelated rules and the global On/Paused
setting are preserved. The original **Create rule → Serve a saved response…** flow still creates
ordinary rules with its established save behavior.

Open **Automation → Experiments** or **Versions and history…**
from the shared response editor. Select an experiment, variant and revision to
inspect them without changing live participation. These selector labels remain
visible while scrolling. Duplicate a saved revision to
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
Activation and resume preserve the global saved-response pause. **Undo activation**
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
response bytes. The [portable investigation APIs](portable-investigations.md#implemented-rule-interchange-apis-k1)
now export selected saved rules/assets and review/import fresh disabled copies
through the UI-neutral Application facade. They preserve authored intent and
source mappings without changing live activation. Desktop file actions, archive
packaging and CLI commands remain later increments.

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

Choose **Automation → Rules → New rule… → Change browser headers…** for User-Agent and client hints. Browser-header rules are part of the shared Rules list; their **Execution order** disclosure contains the ordered browser policies. The first enabled
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
modules. The Rules list shows each browser-header rule and its enabled state.

## Saved responses

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

The primary creation flow starts from a completed traffic item. **Create rule → Serve a saved response…** copies the exact client-visible status, ordered response headers,
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
fields cannot override these choices in the editor. Response text replacement rules create an
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

## Request and response header rules

In **Automation → Rules → New rule…**, choose **Change request headers…** or
**Change response headers…**, select Request or
Response, and use the shared method/address matcher to target the traffic.
New rules start disabled. **Save rule** respects the Enabled choice; **Save and
enable** explicitly applies the saved rule to new matching live requests. The
Saved responses On/Paused switch does not pause header rules. Existing exchanges
retain their captured configuration. Header rules are standalone live automation;
named response variants and captured-page previews currently use response assets.

Operations run in their displayed order. **Set** replaces every matching field
with one field at the first existing position (or appends when absent).
**Append** adds a separate field at the end. **Remove** removes every matching
field. Names match case-insensitively; untouched field order, duplicate values
and bytes are preserved. UTF-8 text and Hex bytes allow non-ASCII field values
without lossy conversion. Invalid names, line breaks and incomplete hex pairs
are rejected while preserving the draft.

Request-header conditions support Exists, Exact, Prefix, Suffix and byte Regex;
response rules also support response-header conditions, status and status class.
A predicate matches when at least one field satisfies it. Conditions use the
effective state at that step, so an earlier rule can affect a later match.
**Test draft** uses the production matcher and operations against explicit sample
headers without sending traffic, executing scripts, saving or activating. It
shows each operation's outcome and ordered before/after fields. An unmatched
draft leaves the sample unchanged. Values can be entered as UTF-8 sample lines;
Hex operation/predicate values preserve bytes that cannot be entered as text.

Removing Content-Security-Policy, adding an Access-Control-Allow-Origin value,
or setting Cache-Control can test a policy hypothesis. These edits alone do not
prove browser behavior. Authored cache policy is preserved. Host, Content-Length,
Transfer-Encoding, Content-Encoding, Connection, Proxy-Connection, Keep-Alive,
TE, Trailer and Upgrade are managed by the proxy and cannot be header-operation
targets; use routing, response-asset or body facilities for those tasks.

| Stage | Order and interaction |
| --- | --- |
| Native request hooks | Earlier to later native rule position. General header operations compose. The first matching User-Agent policy claims that exchange and only replaces existing fields. A later general rule can replace its result. |
| Script request hooks | After native request hooks, in configured script order. |
| Local response | A matching local response stops entering later hooks. Only previously entered hooks can participate in response unwinding. |
| Script response-head hooks | Before native response-head hooks, in reverse script registration order. |
| Native response-head hooks | Earlier to later native response rule position; each rule sees the current effective response and request. |
| Response body planning | After all response-head hooks; matching sees the final effective heads. Script body hooks run in reverse registration order, then native body rules run in saved order. Later body hooks see earlier output. Fresh-response literal edits exclude local responses. |

The **Execution order and conflict repair** disclosure includes native saved responses, User-Agent
policies and advanced rules relevant to the selected phase. Use Earlier/Later
for neighboring header rules or save **Before other native rules** / **After
other native rules** to resolve a shared-position conflict. Overlapping writes
at the same position are rejected atomically. User-Agent policies keep their
separate editor and first-match semantics. No other rule is silently rewritten.
Refresh preserves drafts; a changed saved revision requires reopening that
revision or copying the draft with a new identity. Remove offers Undo until
another automation change makes that restoration stale.

**Traffic → Details → Timings → Recorded header edits** lists actual observed
head-replacement effects in exchange sequence, with captured identities, names,
phases and changed header names. It distinguishes an observed no-op from absent
evidence. It does not infer execution from today's rules or include values.
Original and effective message heads remain the source for header-value evidence.

## Literal edits to fresh responses

Select a request and choose **Actions → Create rule → Find and replace response text…**,
or choose the same action under **Automation → Rules → New rule…**. The disabled draft uses the selected
method/address and a supported media type. It modifies fresh origin text rather
than substituting a saved response. **Save rule** respects Enabled; **Save and
enable** explicitly affects new requests. The Saved responses switch does not pause
these edits. Scoped revisions, position repair and remove/Undo preserve unrelated
automation through the same transaction used by header rules.

Enter case-sensitive literal Find and Replacement text. Replacement is literal,
including `$` characters and an empty string for deletion. First replaces only
the first occurrence; All replaces every non-overlapping occurrence. The optional
required count checks total matches before First/All selection. UTF-8 is the
explicit character encoding in this increment. BOMs, Unicode, line endings and
unrelated text remain unchanged. Regex and structural rewriting are excluded.

Complete decoded input and output must each fit the selected 64 KiB or 1 MiB
bound. Find/replacement are each at most 16 KiB in UTF-8. Existing request and
response matcher conditions apply. Other media types do not select the operation.
A selected response with invalid UTF-8, unsupported/ambiguous charset, partial
range content, incomplete coding, or exceeded bounds fails visibly. A required
count mismatch fails without returning a partial replacement as success. Headers
can already be committed when a body failure occurs; the downstream stream can
fail after an HTTP status was observed. Traffic preserves that distinction.

All response-head hooks finish before body planning. Conditions see final
effective request/response heads. Response body scripts run first in reverse
registration order, followed by native body rules in saved order. Later edits
see earlier body output. Local responses and captured-page previews are excluded;
named variants remain saved response-asset selections.

The shared content pipeline decodes supported codings once and restores the
original coding stack after hooks. It repairs framing and removes stale ETag,
digests and range metadata, preserving Cache-Control and unrelated duplicate
headers. An admitted pipeline can do this even when no literal matches:
**Not applied: text not found** means decoded text is unchanged; encoding bytes
and derived headers can still change. It does not mean exact wire pass-through.
Use header-only rules for validator experiments without a body operation.

**Test draft** uses the production matcher and literal operation against explicit
bounded samples, without traffic, other hooks, saving or activation. Before/after
text stays inert. **Details → Timings → Recorded body edits** shows actual
operation counts, captured identities, decoded lengths and SHA-256 identities,
without body values or authored literals. **Compare original and effective
response** opens the existing upstream/client response comparison. It includes
all intervening changes and reports unavailable/incomplete evidence. Ordinary
body retention applies; an operation hash is not proof of delivery. Interrupted
work can have a selected plan without a completed report.

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
selected saved responses, including disabled rules. Preview participation is
independent of live enabled states and the On/Paused gate. Preview never changes
those states. The same request matcher evaluates actual browser method, normalized
URL and explicit header conditions before captured fallback; the first
participating match wins. Browser headers may differ from captured headers and
captured Vary is not an authored rule condition.

Save editor changes or explicitly exclude unsaved changes before opening a modified
preview. **Preview… → With saved changes…** offers source-local HTML choices for captured
resources, reusing a matching retained scene when available. Authored responses
without a captured source use the last retained scene or selected complete HTML
response. Existing previews and F5 retain their
original rules and linked-file bytes; **Open updated preview…** freezes the new saved
versions. Diagnostics retain historical rule/asset revisions and byte hashes even
when the current saved rule changes or is deleted.

## Compare a response draft

The response editor's **Compare…** menu offers **Draft with saved response**,
**With captured source**, and **With previous saved revision** when its
source/revision is retained. Its **Preview…** menu groups captured baseline and
modified page previews. Save and live activation remain distinct visible actions.
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
edit cannot change an open result. **Preview… → With saved changes…** still uses explicitly
saved rule snapshots; comparing does not save a draft for preview.

See [Traffic comparison](traffic-inspection.md#comparing-captured-evidence) for
representation, coverage, exclusion and processing limits.
