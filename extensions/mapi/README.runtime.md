# MAPI/HTTP visualizer

This extension provides request and response body viewers, a header viewer, and
an optional static MAPI/HTTP traffic classification column. The visible Body
viewer's Auto option tries MAPI/HTTP when its declared header rules match and
shows it after a complete parse. Choose MAPI/HTTP explicitly to inspect partial
results. Traffic-list rendering does not load or execute its parser.

The decoder is authored in TypeScript and compiled to ES2022 JavaScript. It uses
bounded binary readers, protocol modules and a typed tree. It does not include a
.NET runtime, transpiled .NET libraries, a browser, or network access. Transmog
owns the UI and supplies only declared headers, explicitly requested body pages,
and bounded related traffic from the same capture, conversation and origin.

Live updates are optional and begin only when enabled in an active viewer.
Completed interpretations declare that no further data is needed; Transmog then
stops that view's feed and hides Live updates. Imported captures also omit it.
Leaving the visualizer releases its worker, subscriptions and retained data.
Context and body limits can make a parse partial or limited; original retained
bytes remain available in Hex. Missing traffic cannot be reconstructed.
Open ⓘ for parser budget summaries and Copy diagnostics when investigating a
limit. Work is bounded separately for the selected message, paired request and
earlier context, so replaying context cannot consume the selected message's work.

The visualizer has been manually verified to work. Automated qualification uses
the pinned Microsoft Office Inspectors for Fiddler implementation and protocol
specifications as references. Intentional differences
include correct decoded-source ranges, exact UTC FILETIME values, whole
FastTransfer values reassembled across retained chunks, and explicit handling of
reference-parser defects.

Only the two runtime scripts, this README, and the Microsoft MIT notice are
listed for packaging. Authoring sources, tests and tools are not distributed.
The runtime scripts are readable JavaScript. Use Settings → Extensions →
Visualizers and declared inputs → Save to folder to inspect or edit them. Change
the exported manifest's ID (for example, to `local.mapi`) before using Load
unpacked to register it alongside the built-in `net.transmog.mapi` extension.
