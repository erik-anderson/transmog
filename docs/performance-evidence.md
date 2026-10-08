# Request timing and transport evidence

Traffic → Timings shows a snapshot for the selected exchange. Refresh updates
an active request without moving selection. The context menu provides the same
action. All unavailable measurements are explicit.

The live timeline uses one monotonic clock, anchored to complete client request
headers. Wall times are UTC projections of that anchor. Client socket acceptance,
first nonempty socket read, process attribution, and TLS negotiation can precede
headers. On reused connections, these earlier connection events belong to the
physical connection, rather than the current request. A first socket read is not
a measurement of the first packet arriving in the kernel.

The proxy measures routing, upstream assignment, outgoing HTTP body consumption,
response headers/body availability, client-facing response commitment, and body
queue completion. Upload, forwarding, and download can overlap. Assignment wait
includes connection-pool queueing and setup. Response header wait begins after
both assignment and outgoing body consumption; it combines network, server work,
and queues. Early responses may precede body consumption, leaving this phase
unavailable. Local outgoing body consumption does not establish server receipt.
Client queue completion does not establish client receipt or socket flush.

Physical transports have separate identifiers and sampled counters. DNS and TCP
race timings are measured by the Hyper connector. TLS observations include
negotiated version, cipher, ALPN, and session resumption when exposed. Refused TCP,
DNS, and TLS setup attempts retain their measured failure outcome. Setup is shown
as original connection evidence when reused; it is not charged to each stream.
TCP counts socket payload including TLS records, excluding IP/TCP headers and
kernel retransmissions. TCP RTT and loss remain unavailable in this adapter.

HTTP/3 records DNS, combined UDP/QUIC/TLS setup, physical QUIC bytes, active-path
smoothed RTT and congestion window, packets sent/received/lost, and retransmitted
stream bytes. QUIC counters describe all streams on the connection. The adapter
does not expose its cipher or resumption here. Failed QUIC setup still appears in
route-attempt diagnostics; its separate physical measurements are unavailable.

Native format revision 3 retains bounded timing, transport and actual HTTP
boundary protocol evidence. Readers continue accepting revisions 1 and 2, with
missing evidence left unavailable. Delayed observer snapshots cannot erase later
terminal milestones or newer connection samples. Imported SAZ SessionTimers and
session attributes remain inspectable under their original names and clocks.
No zero or unset imported timestamp is inferred to be a measured event.
