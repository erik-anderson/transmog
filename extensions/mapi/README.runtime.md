# MAPI/HTTP visualizer

This extension provides request and response body viewers, a header viewer, and
an optional static MAPI/HTTP traffic classification column. Choose MAPI/HTTP in
the relevant viewer selector to activate it. Ordinary traffic browsing does not
load or execute its parser.

The decoder is authored in TypeScript and compiled to ES2022 JavaScript. It uses
bounded binary readers, protocol modules and a typed tree. It does not include a
.NET runtime, transpiled .NET libraries, a browser, or network access. Transmog
owns the UI and supplies only declared headers, explicitly requested body pages,
and bounded related traffic from the same capture, conversation and origin.

Live updates are optional and begin only when enabled in an active viewer.
Leaving the visualizer releases its worker, subscriptions and retained data.
Context and body limits can make a parse partial or limited; original retained
bytes remain available in Hex. Missing traffic cannot be reconstructed.

Synthetic qualification uses the pinned Microsoft Office Inspectors for Fiddler
implementation and protocol specifications as references. Intentional differences
include correct decoded-source ranges, exact UTC FILETIME values, whole
FastTransfer values reassembled across retained chunks, and explicit handling of
reference-parser defects. Real Office capture validation is deferred; no such
end-to-end validation is claimed.

Only the two runtime scripts, this README, and the Microsoft MIT notice are
listed for packaging. Authoring sources, tests and tools are not distributed.
