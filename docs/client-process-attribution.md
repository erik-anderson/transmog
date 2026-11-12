# Client process attribution

Transmog resolves a best-effort snapshot of the client process that opened each
downstream TCP connection. A resolved local caller contains the process name and
PID. A non-loopback caller is recorded as `remote`; a loopback caller that
cannot be resolved is recorded as `local-unknown`.

Attribution runs once, immediately after the listener accepts the connection.
For inspected connections, the result is copied into each exchange's immutable session metadata, so core
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
  `/proc/<pid>/fd` links for its owner. Names come from `/proc/<pid>/exe`;
  inaccessible images remain nameless. The mutable, truncated `comm` value
  cannot reliably identify an executable for inspection exclusions.
  PID namespaces, mount namespaces, and
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
processes. Their source IP is also preserved in request metadata and shown in the
inspector; loopback clients do not display that extra field. This both communicates the useful fact and avoids implying that a
remote PID would be meaningful on the proxy host.

## Reliability and security

Process attribution is diagnostic context, never an authentication or
authorization input. Socket tables are point-in-time data: a very short-lived
connection can disappear before lookup, a process can exit, and a PID can later
be reused. Access restrictions can also prevent reading a process name even when
the PID is visible. These cases never block proxy traffic and degrade to an
unknown name or `local-unknown` classification.

Process inspection uses this snapshot to choose whether a connection enters
inspection. **All processes** is the default. With **Only these processes** or
**All except these processes**, a caller with an unavailable name, an unresolved
local identity, or a remote identity passes through without inspection. This
avoids intercepting an excluded application when its name cannot be read.
The runtime exposes an aggregate unresolved-caller count without retaining
those callers' requests or destinations. Existing remote-access restrictions
still apply before admission.

These choices are debugging controls, not access controls. Executable names
can be shared or changed, and attribution is best effort. See
[process inspection](network-profiles.md#choose-processes-to-inspect) for exact
matching, capture, and connection-lifetime behavior.

The settings process picker uses a separate, bounded snapshot of readable
running executable names. It returns no full paths or PIDs, merges duplicate
names, and does not require an application to be running before its name can
be entered. Live connection decisions never depend on the suggestion inventory.

All operating-system scans are bounded. The runtime also contains resolver
panics and substitutes `local-unknown` (or `remote` for a non-loopback peer), so
a custom resolver cannot take down the accept loop.

[Native TMCap](native-capture-format.md) preserves the connection identity and
source IP for later inspection and conversion.
