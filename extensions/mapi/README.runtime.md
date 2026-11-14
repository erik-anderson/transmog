# MAPI/HTTP visualizer

This extension adds request and response body viewers, a header viewer and an
optional MAPI/HTTP traffic column. Body Auto selects this viewer when matching
input parses completely. Choose MAPI/HTTP explicitly to inspect partial results.
Traffic-list rendering does not run the parser.

Transmog supplies only declared headers, requested body pages and related traffic
from the same capture, conversation and origin. The extension has no filesystem,
network or browser access. Size and structure checks bound input and output;
original retained bytes remain available in Hex. Open ⓘ to read diagnostics,
or choose Copy diagnostics to copy them.

Live updates begin only when enabled in an active viewer. A complete body
interpretation of complete input stops that viewer's feed and hides Live updates.
Imported captures do not offer Live updates. Leaving the visualizer releases its
worker, subscriptions and retained data. Missing traffic cannot be reconstructed.

The package contains readable ES2022 JavaScript, source maps with embedded
TypeScript, this guide and the Microsoft MIT notice. Browse the TypeScript and
set mapped breakpoints in the debugger's Sources pane. Tests and compiler tools
are excluded.

To inspect or edit a copy, use Settings → Extensions → Visualizers and declared
inputs → Save to folder…. Change its manifest ID, for example to `local.mapi`,
before using Load unpacked… alongside the bundled `net.transmog.mapi` extension.
After editing JavaScript directly, remove `sourceMaps` from the copied manifest
so the debugger shows the edited source.
