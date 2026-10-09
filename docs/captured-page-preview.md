# Preview a captured page

On Windows, select an HTML response in Traffic and choose **Preview page…**.
Read the warning and choose whether to **Enable scripts for this preview**.
Scripts start disabled each time. **Open preview** opens a separate WebView2
window; the Transmog proxy can remain stopped. Cancel or Escape stops preparation.

Traffic marks identifiable top-level navigations with a subtle **Page** badge
beside the path. This uses the original request's `Sec-Fetch-Dest: document`
and, when present, `Sec-Fetch-Mode: navigate` as defined by
[Fetch Metadata](https://www.w3.org/TR/fetch-metadata/#sec-fetch-dest-header).
Iframe requests and entries without clear fetch metadata remain unmarked;
HTML content alone does not identify a top-level navigation.

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
limits still apply; unknown URLs return empty 404s. Accept-Encoding is ignored
after response decoding. The preview keeps the selected request's recorded
User-Agent, when available, to help scripts behave like the original browser.
Rendering a selected POST response provides its body for initial GET navigation.
Cache-dependent 304 responses are skipped in favor of an available captured
representation for that URL; the preview's fresh browser profile has no original
cache to satisfy them.

This recreates captured evidence rather than a complete browser session.
Missing, incomplete or oversized resources remain unavailable. Original response
policies such as CSP still apply. Browser state from the recording device is
unavailable. Service workers, WebSockets, WebTransport, peer connections,
downloads, popups, device permissions and authentication prompts are disabled.
The native context menu and DevTools are available. F5 reloads the frozen page;
a failed top-level navigation closes the preview.
The initial implementation is Windows-only; other platforms require their own
resource-interception boundary.

## Preview diagnostics

**Last preview diagnostics** stays in the trusted workbench and belongs to the
window that opened the preview. It shows which captured variant was chosen,
links to retained source traffic, served/missing counts and script choice. Refresh
after navigating the preview. Reports remain readable after the preview closes.
Recent reports and displayed observations are bounded; counts include requests
that no longer fit in the displayed list. Headers and request bodies are excluded.

Preparation bounds decoded resource bytes, request-body hashing and variants.
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

Captured content can still be malicious. The warning and optional script choice
are part of the flow even with isolation; use a current WebView2 runtime and
preview content from a source you trust.

Scene tests cover trace isolation, nearest-response selection, URI normalization,
missing resources and cancellation. Browser checks exercise the warning, scripts
choice, Escape and compact layout. The native viewer probe uses a captured
HTML/CSS/image fixture, verifies both script choices, empty missing responses,
blocked app commands and zero requests to a controlled uncaptured endpoint.
