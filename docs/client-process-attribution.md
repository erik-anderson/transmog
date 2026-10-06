# Client process attribution

Transmog records a best-effort snapshot of the client process that opened each
downstream TCP connection. A resolved local caller contains the process name and
PID. A non-loopback caller is recorded as `remote`; a loopback caller that
cannot be resolved is recorded as `local-unknown`.

Attribution runs once, immediately after the listener accepts the connection.
The result is copied into every exchange's immutable session metadata, so core
hooks, observers, the application session catalog, and TMCap capture records see
the same value. The desktop traffic table presents it in the **Caller** column.

## Platform behavior

- **Windows:** the resolver enumerates the owning-PID TCP table with
  [`GetExtendedTcpTable`](https://learn.microsoft.com/windows/win32/api/iphlpapi/nf-iphlpapi-getextendedtcptable),
  matches the complete client-to-proxy IPv4 or IPv6 tuple, and queries the
  process image using query-only process access. Only the image's file name is
  retained.
- **Linux:** the resolver matches the complete tuple in `/proc/net/tcp` or
  `/proc/net/tcp6`, obtains its socket inode, and searches the visible
  `/proc/<pid>/fd` links for its owner. The name comes from `comm`, with the
  executable file name as a fallback. PID namespaces, mount namespaces, and
  `/proc` access controls can intentionally hide the owner.
- **macOS and other platforms:** loopback clients currently resolve as
  `local-unknown`. macOS exposes `proc_pidfdinfo`, but Apple labels the
  [`libproc` interfaces](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.h)
  private and subject to change. The built-in resolver does not make a
  production dependency on that unstable ABI. An embedding application can
  supply a platform resolver with
  `ProxyComponents::with_client_identity_resolver` without changing the core
  exchange model.

Non-loopback clients are classified as `remote` without enumerating host
processes. This both communicates the useful fact and avoids implying that a
remote PID would be meaningful on the proxy host.

## Reliability and security

Process attribution is diagnostic context, never an authentication or
authorization input. Socket tables are point-in-time data: a very short-lived
connection can disappear before lookup, a process can exit, and a PID can later
be reused. Access restrictions can also prevent reading a process name even when
the PID is visible. These cases never block proxy traffic and degrade to an
unknown name or `local-unknown` classification.

All operating-system scans are bounded. The runtime also contains resolver
panics and substitutes `local-unknown` (or `remote` for a non-loopback peer), so
a custom resolver cannot take down the accept loop.

Caller identity was added in TMCap format revision 2. The native streaming
capture format therefore preserves the connection snapshot for later
inspection and conversion.
