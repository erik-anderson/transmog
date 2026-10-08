# Preview a captured page

On Windows, select an HTML response in Traffic and choose **Preview page…**.
Read the warning and choose whether to **Enable scripts for this preview**.
Scripts start disabled each time. **Open preview** opens a separate WebView2
window; the Transmog proxy can remain stopped. Cancel or Escape stops preparation.

The page keeps its original URL so relative CSS, images and other resources can
resolve naturally. Resources come from the selected entry's original trace,
including when several traces have been imported into one workspace. For each
method and URL, the nearest captured response to the selected request is used.
Rendering a selected POST response provides its body for the initial GET used
by the browser. Unmatched requests return **404 with an empty body**.

This recreates captured evidence rather than a complete browser session.
Missing, incomplete or oversized resources remain unavailable. Original response
policies such as CSP still apply. Browser state from the recording device is
unavailable. Service workers, WebSockets, WebTransport, peer connections,
downloads, popups, device permissions and authentication prompts are disabled.
The initial implementation is Windows-only; other platforms require their own
resource-interception boundary.

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
