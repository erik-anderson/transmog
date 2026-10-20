# MAPI/HTTP visualizers

Choose **MAPI/HTTP** in the request or response body viewer to inspect mailbox,
address-book and FastTransfer structures. **MAPI/HTTP headers** interprets only
the declared headers and does not load the binary decoder. The app owns all
tree, table, text and Hex presentation. Expand a structure or choose a byte range
to inspect its retained or decoded source.

Body decoding may read up to 64 earlier retained exchanges from the same capture,
conversation and origin, within an 8 MiB snapshot. Missing or incomplete context
is reported as such. Live updates are optional and start off; leaving the
visualizer stops its worker and retained-input feed. Scripts cannot access the
network, files or other captures directly.

The decoder is based on Microsoft's Office Inspectors for Fiddler, revision
7a76470574377d14cae612c507f45b34d70e05a0. Qualification uses synthetic protocol
data, including native differential and sandbox tests. Testing with a real Office
capture is deferred. The package retains the Microsoft MIT license, Transpose's
Apache 2.0 license and the embedded runtime notices in `LICENSES/`.
