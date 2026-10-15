# Preview a captured page

On Windows, select an HTML response in Traffic and choose **Preview page…**.
Read the warning and choose whether to **Enable scripts for this preview**.
Scripts start disabled each time. **Open preview** opens a separate WebView2
window; the Transmog proxy can remain stopped. Cancel or Escape stops preparation.

In the main window, **Responses** defaults to **Captured**. Choose **Captured with
auto-responses** and expand **Select saved rules in priority order** to include
saved rules. Enabled rules are selected initially; disabled rules may also
participate. This choice is **For this preview only**: it does not enable live
rules, and the live On/Paused switch does not gate preview participation. Resolve
an editor draft by saving it or explicitly exclude its unsaved changes. The
editor's **Preview captured page…** shortcut uses the last retained preview scene,
or the selected complete HTML response in Traffic.

The first participating rule matching the actual browser method, normalized URL
and explicit request-header conditions supplies its saved response. It can supply
a URL absent from the capture. The matcher is shared with live rules and editor
tests; captured Vary does not add a rule predicate. Header-dependent rules cannot
match when native request-header inspection is unavailable. Native headers over
the matcher's 64-field/32 KiB budget make rule evaluation unavailable, which is
reported beside captured fallback. Browser-managed headers may differ from the
original capture; no captured header values are substituted into rule matching.

Navigation starts with GET even for a selected POST response. A POST-only rule
does not match that GET; the selected POST response remains the captured fallback.
Edit the authored method explicitly when a GET override is intended.

Opening freezes rule order, participation, revisions and exact response bytes,
including bounded copies of linked files. A changed, unreadable or oversized
selected asset blocks preparation with the rule's name. Rule edits, deletion,
reordering and external-file changes cannot alter an existing preview or F5.
**Open baseline…** and **Open updated preview…** in Actions or diagnostics reuse
the same frozen captured scene, while the updated preview snapshots current saved
rules. Source scope stays fixed for related previews; scripts remain an independent
choice. Titles and reports distinguish baseline/modified state and script choice.

Traffic marks identifiable top-level navigations with a subtle **Page** badge
beside the path. This uses the original request's `Sec-Fetch-Dest: document`
and, when present, `Sec-Fetch-Mode: navigate` as defined by
[Fetch Metadata](https://www.w3.org/TR/fetch-metadata/#sec-fetch-dest-header).
Iframe requests and entries without clear fetch metadata remain unmarked;
HTML content alone does not identify a top-level navigation.

Use **Filter → Fetch destination** to narrow traffic by its original
`Sec-Fetch-Dest` header. The checkbox dropdown accepts multiple destinations;
matching any selected value includes an entry. The selection combines with other
filters and content-search results across all retained entries before virtual windowing.
Clear the destination chip or all filters to restore the list. **Fetch / XHR
(empty)** matches the recorded `empty` value; missing headers remain unknown.

The page keeps its original URL so relative CSS, images and other resources can
resolve naturally. Resources come from the selected entry's original trace,
including when several traces have been imported into one workspace. For each
method and URL, preview prefers matching request-body bytes and fetch destination,
the recorded User-Agent and top-level process, Origin/Referer and other Sec-
headers, then Vary and capture timing. Process identity is scoped to its capture
so a reused PID from another file is not treated as the same process. Responses
at or after the top-level navigation are preferred over earlier responses when
other hints tie. When a URL is requested again, selection advances to a later
captured response if one is available. Otherwise it reuses the closest eligible
response. Reloading the selected document resets this per-load sequence.

**All loaded traffic in this window** can mix traces or users while keeping the
selected HTML fixed. Hints are relaxed when no perfect match exists: an available
response for the same method and URL is still served even with different or
unavailable bodies, Vary, UA or other headers. Source scope and complete-response
limits still apply; unknown URLs produce network failures. Missing capture bytes
are not evidence of an original server 404: `fetch()` rejects and resource loads
raise errors. The private deny endpoint closes the connection without an HTTP
response and never contacts an origin. Genuine captured or authored HTTP error
responses, including 404, keep their status. Accept-Encoding is ignored
after response decoding. The preview keeps the selected request's recorded
User-Agent, when available, to help scripts behave like the original browser.
Rendering a selected POST response provides its body for initial GET navigation.
Cache-dependent 304 responses are skipped in favor of an available captured
representation for that URL; the preview's fresh browser profile has no original
cache to satisfy them.

This recreates captured evidence rather than a complete browser session.
Missing, incomplete or oversized resources remain unavailable. Original response
policies such as CSP still apply. Auto-response CSP, CORS, content type and cache
headers remain effective alongside mandatory preview isolation. Content coding
and framing are repaired after decoding; HEAD and no-body statuses deliver no
body. An unsupported no-body document response fails navigation instead of
becoming a successful page. Browser state from the recording device is
unavailable. Service workers, WebSockets, WebTransport, peer connections,
downloads, popups, device permissions and authentication prompts are disabled.
The native context menu and DevTools are available. F5 reloads the frozen page;
a failed top-level navigation closes the preview.
The preview is Windows-only; other platforms require their own
resource-interception boundary.

## Preview diagnostics

**Last preview diagnostics** stays in the trusted workbench and belongs to the
window that opened the preview. It shows which captured variant was chosen,
links to retained source traffic and current saved rules, served/missing/failed
counts, script choice and WebView2 version. Frozen rule revisions, asset references
and SHA-256 hashes identify historical bytes even after their sources change.
Rule links open the current saved rule; the report retains the historical revision.
Reports identify relaxed capture hints and rule condition results without exposing
header values or request bodies. Refresh
after navigating the preview. Reports remain readable after the preview closes.
Up to eight reports remain, with up to 256 resource decisions and the last 256
requests each. Truncation counts include omitted observations. At most three
recent runs retain scene bytes for related previews; older reports explain expired
bytes and direct you to select HTML again. Closing the opening window releases its
reports and closes its previews. Counters include requests that no longer fit in
the displayed list. Headers and request bodies are excluded.

Preparation allows 2,048 captured variants, 32 MiB per decoded resource and
256 MiB of decoded capture plus rule bytes per run, with at most 64 selected rules.
It also bounds request-body hashing.
Oversized or unavailable resources remain missing; these processing budgets do
not constrain capture or file-based Composer replay. The
[preparation code](../crates/app/src/captured_page_build.rs) owns these limits.

## Isolation and implementation

Captured HTML is never rendered in the trusted workbench. Each preview starts
with a fresh temporary browser profile. It cannot invoke application commands,
use host objects or exchange app messages. An audited COM adapter installs
all-context, all-source WebView2 request interception before navigating away
from a hidden `about:blank` window. Unsupported runtimes fail before showing
content. Captured response files are decoded and bounded off the UI thread.

A private deny-only loopback endpoint also blocks browser transport fallback;
it never contacts origins, changes system proxy settings or installs roots.
Render-only CSP blocks workers and privileged content, and every frame receives
transport API restrictions. Normal app protocols reject preview window labels.
The preview owns its response files and profile until its browser releases them.
Profile deletion retries in the background while WebView2 releases file locks.
At most three previews may be preparing or open at once.
Capture viewers continue to offer captured baselines. To use workspace rules,
import the capture through the main window; viewers cannot query its automation
or request an overlay through the host command.

Captured content can still be malicious. The warning and optional script choice
are part of the flow even with isolation; use a current WebView2 runtime and
preview content from a source you trust.

Scene tests cover trace isolation, nearest-response selection, URI normalization,
missing resources and cancellation. Browser checks exercise the warning, scripts
choice, Escape and compact layout. The native viewer probe uses a captured
HTML/CSS/image fixture, verifies both script choices, rejected missing requests,
blocked app commands and zero requests to a controlled uncaptured endpoint.
