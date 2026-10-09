# Request timings and transport evidence

Select a request and choose **Timings** or **Timings and transport…** in its
context menu. The report is a snapshot; **Refresh** updates an active request
without changing selection. The waterfall compares overlapping intervals,
local operations and body milestones. **Copy report** includes the timeline,
physical connection facts, source association and original imported timers.
The report stays available for manual copying if clipboard access fails.

## Interpret latency

One monotonic clock is anchored to complete incoming request headers. Wall times
are UTC projections of that anchor. Client connection acceptance, first nonempty
socket read, process attribution and TLS negotiation can precede zero. On reused
connections these observations belong to the physical connection, not the
current request. A socket read does not measure first packet arrival in the kernel.

The proxy observes routing, upstream assignment, outgoing body consumption,
response headers/body availability, client response commitment and queue
completion. Upload, forwarding and download can overlap. Assignment wait includes
pool queueing and setup. Response header wait combines network, server work and
queues; it does not isolate server CPU time. Early responses can precede outgoing
body completion and leave a derived phase unavailable.

Local body consumption does not establish server receipt. Client queue completion
does not establish client receipt or socket flush. First-body markers identify
adapter observations and queue acceptance. Zero means a measured zero;
unavailable means there is no measurement.

Named operation rows show breakpoint decisions, hook/script callbacks, codec
work and forwarding-channel waits. Local measurements below 1 ms are in the
collapsed **Proxy operations under 1 ms** section; expand it to inspect them. Latency phase
details and the event timeline are also expandable. Repeated callback windows have dashed borders;
their call time sums observed invocations and excludes gaps. It includes nested
work and waits, so it is not CPU time and rows need not add up to total duration.

## Shared connection setup

DNS, TCP, TLS and combined QUIC/TLS phases retain their original start and finish
on the request's clock. For pooled or multiplexed connections, the request is
charged only for its overlap with setup after upstream admission. A phase already
completed shows zero request wait, alongside its original duration and relative
start/finish offsets. Those offsets show how long before this request the shared
connection started.

If a request joins setup still in progress, the report shows both its own wait
and the full connection phase duration. Missing source timestamps remain
unavailable; a shared duration alone cannot establish a request's wait or the
connection's age. Failed DNS, TCP and TLS attempts preserve their measured
outcome, while route-attempt diagnostics explain failed QUIC setup.

## Transport observations

Physical connections have separate identifiers and sampled counters. TLS facts
include negotiated version, cipher, ALPN and session resumption when the adapter
exposes them. TCP socket byte counts include TLS records but exclude IP/TCP
headers and kernel retransmissions. They describe transport activity rather than
HTTP entity size.

Windows SIO_TCP_INFO and Linux TCP_INFO provide read-only kernel observations
where supported. Available RTT, window and retransmission fields appear under
Physical connections; unsupported fields are omitted. RTT/windows are snapshots,
retransmissions are cumulative, and multiplexed streams share the counters. The
kernel sample time is distinct from report time. Local write/flush waits indicate
backpressure rather than remote acknowledgement. Sampling does not enable OS
instrumentation or require elevation.

HTTP/3 observations include DNS, combined UDP/QUIC/TLS setup, physical QUIC bytes,
active-path smoothed RTT, congestion window, packet counts and retransmitted
stream bytes where exposed. These counters describe all streams on the connection.
Separate failed-setup transport details, cipher and session resumption may be
unavailable.

## Saved evidence and source of truth

TMCap retains measured timing, transport and actual HTTP boundary protocols.
SAZ SessionTimers, attributes and flags remain inspectable under their original
names and clocks. Zero or unset imported timestamps are not inferred to be events.
See [native encoding](native-capture-format.md) and [SAZ compatibility](saz-compatibility.md).

The [performance model](../crates/proxy-core/src/performance.rs) defines milestone
semantics, setup projection and evidence merging. Adapter code owns the point
where each measurement is made; adding a field alone does not establish remote
receipt or packet-level timing.
