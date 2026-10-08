//! Measured proxy milestones and physical transport evidence. Timing does not
//! assert receipt by a remote peer or identify time spent executing on a server.
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// A local observation point, rather than an inferred remote event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Milestone {
    /// The proxy accepted the client's TCP socket.
    ClientConnected,
    /// First nonempty socket read on the client's physical connection.
    ClientFirstByte,
    /// Best-effort client process attribution completed before HTTP serving.
    ClientIdentityDone,
    /// The client's TLS handshake began.
    ClientTlsBegin,
    /// The client's TLS handshake completed.
    ClientTlsDone,
    /// Complete client request headers became available.
    RequestHeaders,
    /// Client request body ended at the ingress adapter.
    ClientRequestDone,
    /// Routing policy selection began.
    RouteBegin,
    /// Routing policy selection completed.
    RouteDone,
    /// The effective request was submitted to an upstream adapter.
    UpstreamBegin,
    /// An upstream physical connection was assigned to the request.
    UpstreamConnected,
    /// The upstream HTTP adapter consumed the final outgoing request body frame.
    UpstreamRequestConsumed,
    /// The first nonempty request body was queued for the upstream HTTP adapter.
    UpstreamRequestFirstBodyQueued,
    /// First nonempty response body exposed by the upstream HTTP adapter.
    UpstreamResponseFirstBody,
    /// First nonempty response body queued for the client HTTP adapter.
    ClientResponseFirstBodyQueued,
    /// Complete upstream response headers became available.
    ResponseHeaders,
    /// The upstream response body ended at the adapter.
    UpstreamResponseDone,
    /// Effective response headers were committed to the client adapter.
    ClientResponseBegin,
    /// The effective response body finished being queued for the client.
    ClientResponseQueued,
    /// Successful or failed exchange processing ended.
    ExchangeDone,
}

/// A wall-clock anchor plus monotonic time relative to request headers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimingPoint {
    /// The event that was actually observed.
    pub milestone: Milestone,
    /// UTC Unix milliseconds; derived from the initial wall-clock anchor.
    pub unix_millis: u64,
    /// Monotonic microseconds relative to complete incoming request headers.
    /// Connection observations can precede the request and have negative offsets.
    pub offset_micros: i64,
}

/// Outcome of an observed physical setup attempt.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportOutcome {
    /// The adapter did not expose an outcome.
    #[default]
    Unavailable,
    /// The physical connection was established.
    Connected,
    /// DNS failed before TCP connection attempts.
    DnsFailed,
    /// TCP connection attempts failed.
    TcpFailed,
    /// TLS negotiation or certificate verification failed.
    TlsFailed,
}

/// Request-relative view of one physical connection setup phase.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionTiming {
    /// dns, tcp, tls, or quic (QUIC transport and TLS together).
    pub phase: String,
    /// Original connection phase start relative to this request's header clock.
    pub began_offset_micros: i64,
    /// Original phase end on that same monotonic clock.
    pub ended_offset_micros: i64,
    /// Time this request actually overlapped the phase after upstream admission.
    pub request_wait_micros: u64,
}

/// Adapter-owned monotonic setup measurements; never stored as a foreign clock.
#[derive(Clone, Debug)]
pub struct ConnectionSetupTime {
    /// dns, tcp, tls, or quic.
    pub phase: &'static str,
    /// Physical phase start, shared across all uses of this connection.
    pub began: Instant,
    /// Physical phase completion.
    pub ended: Instant,
}

/// Measurable facts from a physical connection, shared by HTTP/2 streams.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransportObservation {
    /// Monotonic sampling point, allowing consumers to reject older snapshots.
    #[serde(default)]
    pub sampled_offset_micros: i64,
    /// Actual setup outcome; unavailable is distinct from a zero duration.
    pub outcome: TransportOutcome,
    /// `client` or `upstream` leg.
    pub leg: String,
    /// Physical connection identity, independent of stream identities.
    pub connection_id: String,
    /// Peer endpoint, when measurable.
    pub peer: Option<String>,
    /// Local endpoint, when measurable.
    pub local: Option<String>,
    /// True when the same physical connection has served another exchange.
    pub shared: bool,
    /// Physical phase timestamps projected onto this request, with its own wait.
    #[serde(default)]
    pub setup_timings: Vec<ConnectionTiming>,
    /// Request-specific DNS wait with setup timings; legacy captures may hold
    /// the physical duration. Absent when no lookup occurred.
    pub dns_micros: Option<u64>,
    /// Request-specific TCP race wait with setup timings; otherwise the physical
    /// duration, separate from DNS and TLS.
    pub tcp_micros: Option<u64>,
    /// Request-specific TLS/QUIC wait with setup timings; otherwise physical
    /// duration. Absent for plaintext or unavailable adapters.
    pub tls_micros: Option<u64>,
    /// Negotiated TLS protocol, without any key material.
    pub tls_version: Option<String>,
    /// Whether the TLS session was resumed, when exposed by the adapter.
    pub tls_resumed: Option<bool>,
    /// Negotiated cipher suite.
    pub cipher: Option<String>,
    /// Negotiated application protocol.
    pub alpn: Option<String>,
    /// Physical bytes read at the observation point; includes all shared streams.
    pub bytes_read: Option<u64>,
    /// Physical bytes written at the observation point; includes all shared streams.
    pub bytes_written: Option<u64>,
    /// Latest kernel TCP snapshot; absent if unsupported or on UDP/QUIC.
    #[serde(default)]
    pub tcp: Option<TcpObservation>,
    /// Actual kernel snapshot time on this request's clock (may precede the report).
    #[serde(default)]
    pub tcp_sampled_offset_micros: Option<i64>,
    /// Physical socket write/flush evidence, independent of HTTP body queues.
    #[serde(default)]
    pub socket_io: Option<SocketIoObservation>,
    /// QUIC recovery/path statistics; absent on TCP adapters.
    #[serde(default)]
    pub quic: Option<QuicObservation>,
}

/// Read-only kernel TCP snapshot. Counters and windows belong to the whole
/// physical connection, including every multiplexed request. None is unavailable.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TcpObservation {
    /// Kernel estimated round trip in microseconds.
    pub rtt_micros: Option<u64>,
    /// Kernel minimum round trip in microseconds, when exposed.
    pub min_rtt_micros: Option<u64>,
    /// Congestion window in bytes.
    pub congestion_window: Option<u64>,
    /// Peer advertised send window in bytes.
    pub send_window: Option<u64>,
    /// Local advertised receive window in bytes.
    pub receive_window: Option<u64>,
    /// Sent bytes not acknowledged at this snapshot.
    pub unacknowledged_bytes: Option<u64>,
    /// Cumulative bytes retransmitted, when exposed.
    pub retransmitted_bytes: Option<u64>,
    /// Cumulative retransmitted segments, when exposed.
    pub retransmitted_segments: Option<u64>,
    /// Cumulative fast retransmission episodes.
    pub fast_retransmissions: Option<u64>,
    /// Cumulative duplicate acknowledgments received.
    pub duplicate_acks: Option<u64>,
    /// Cumulative timeout episodes.
    pub timeout_episodes: Option<u64>,
    /// Maximum segment size in bytes.
    pub mss: Option<u64>,
    /// Kernel connection age in milliseconds.
    pub connection_age_millis: Option<u64>,
}

/// Local socket I/O evidence; successful writes mean kernel acceptance,
/// never peer receipt or acknowledgment. All values are shared connection totals.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SocketIoObservation {
    /// Sum of time pending on physical writes or flushes, including an ongoing wait.
    pub write_wait_micros: u64,
    /// Number of distinct pending write/flush episodes.
    pub write_waits: u64,
    /// Last nonempty write accepted by the socket, on this request's clock.
    pub last_write_offset_micros: Option<i64>,
    /// Last successful local flush, on this request's clock; not remote receipt.
    pub last_flush_offset_micros: Option<i64>,
}

/// Physical QUIC counters, shared by all streams on this connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuicObservation {
    /// Active-path smoothed round trip time.
    pub rtt_micros: Option<u64>,
    /// Active-path congestion window in bytes.
    pub congestion_window: Option<u64>,
    /// Packets sent, including retransmitted data.
    pub packets_sent: u64,
    /// Packets received.
    pub packets_received: u64,
    /// Packets declared lost by recovery.
    pub packets_lost: u64,
    /// Stream bytes retransmitted by recovery.
    pub retransmitted_bytes: u64,
}

/// Actual HTTP version observed at a message boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolObservation {
    /// Stable boundary name, such as `client-request`.
    pub boundary: String,
    /// Exact HTTP version, including the HTTP/1 minor version when known.
    pub version: String,
    /// Actual reason phrase when the transport exposes it.
    pub reason: Option<String>,
}

/// Aggregated measured local work. The window can include gaps between calls;
/// busy time is the sum of actual observed calls, including their own waits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkTiming {
    /// Stable work category, such as hook, transform, decode, encode or pause.
    pub kind: String,
    /// Bounded operation or interceptor name.
    pub label: String,
    /// First observed call start on the request's clock.
    pub began_offset_micros: i64,
    /// Last observed call completion on that clock.
    pub ended_offset_micros: i64,
    /// Sum of measured call time in nanoseconds, preserving sub-microsecond work.
    pub busy_nanos: u64,
    /// Number of measured invocations represented by this row.
    pub calls: u64,
}

/// Records an operation even when its future is canceled or fails.
#[derive(Debug)]
pub struct WorkTimer {
    recorder: PerformanceRecorder,
    kind: &'static str,
    label: String,
    began: Instant,
}
impl Drop for WorkTimer {
    fn drop(&mut self) {
        self.recorder
            .record_work(self.kind, &self.label, self.began, Instant::now());
    }
}

/// Bounded point-in-time evidence carried by observers and native captures.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerformanceEvidence {
    /// At most one first observation of each milestone.
    pub points: Vec<TimingPoint>,
    /// At most sixteen physical transport observations, for fallback attempts.
    pub transports: Vec<TransportObservation>,
    /// At most four actual message boundary protocols.
    pub protocols: Vec<ProtocolObservation>,
    /// At most 128 named work groups, including an overflow group.
    #[serde(default)]
    pub work: Vec<WorkTiming>,
}
impl PerformanceEvidence {
    /// Processing duration from incoming headers to the local terminal event.
    pub fn elapsed_micros(&self) -> Option<u64> {
        let start = self
            .points
            .iter()
            .find(|point| point.milestone == Milestone::RequestHeaders)?;
        let end = self
            .points
            .iter()
            .find(|point| point.milestone == Milestone::ExchangeDone)?;
        u64::try_from(end.offset_micros.checked_sub(start.offset_micros)?).ok()
    }
    /// Validates the finite shape before storing evidence from a saved file.
    pub fn valid(&self) -> bool {
        self.points.len() <= 24
            && self.transports.len() <= 16
            && self.protocols.len() <= 4
            && self.work.len() <= 128
            && self.work.iter().all(|work| {
                work.kind.len() <= 256
                    && work.label.len() <= 512
                    && work.calls > 0
                    && work.ended_offset_micros >= work.began_offset_micros
            })
            && self.points.iter().enumerate().all(|(index, point)| {
                point.unix_millis <= 253_402_300_799_999
                    && !self.points[..index]
                        .iter()
                        .any(|other| other.milestone == point.milestone)
            })
            && self.transports.iter().all(|transport| {
                matches!(transport.leg.as_str(), "client" | "upstream")
                    && transport.connection_id.len() <= 128
                    && transport.setup_timings.len() <= 4
                    && transport.setup_timings.iter().all(|timing| {
                        matches!(
                            timing.phase.as_str(),
                            "dns" | "tcp" | "tls" | "quic" | "certificate"
                        ) && timing
                            .ended_offset_micros
                            .checked_sub(timing.began_offset_micros)
                            .is_some_and(|elapsed| {
                                elapsed >= 0
                                    && timing.request_wait_micros <= elapsed.cast_unsigned()
                            })
                    })
                    && [
                        &transport.peer,
                        &transport.local,
                        &transport.tls_version,
                        &transport.cipher,
                        &transport.alpn,
                    ]
                    .into_iter()
                    .all(|field| field.as_ref().is_none_or(|value| value.len() <= 512))
            })
            && self.protocols.iter().all(|protocol| {
                matches!(
                    protocol.boundary.as_str(),
                    "client-request" | "upstream-request" | "upstream-response" | "client-response"
                ) && matches!(
                    protocol.version.as_str(),
                    "HTTP/1.0" | "HTTP/1.1" | "HTTP/2" | "HTTP/2.0" | "HTTP/3" | "HTTP/3.0"
                ) && protocol
                    .reason
                    .as_ref()
                    .is_none_or(|reason| reason.len() <= 1024)
            })
    }
    /// Merges coalesced snapshots without allowing a delayed observer update to
    /// erase later milestones or replace newer physical counter samples.
    pub fn merge(&mut self, incoming: &Self) {
        if !incoming.valid() {
            return;
        }
        for point in &incoming.points {
            if self.points.len() < 24
                && !self
                    .points
                    .iter()
                    .any(|existing| existing.milestone == point.milestone)
            {
                self.points.push(point.clone());
            }
        }
        for transport in &incoming.transports {
            if let Some(existing) = self.transports.iter_mut().find(|existing| {
                existing.leg == transport.leg && existing.connection_id == transport.connection_id
            }) {
                if transport.sampled_offset_micros >= existing.sampled_offset_micros {
                    *existing = transport.clone();
                }
            } else if self.transports.len() < 16 {
                self.transports.push(transport.clone());
            }
        }
        for work in &incoming.work {
            if let Some(existing) = self
                .work
                .iter_mut()
                .find(|existing| existing.kind == work.kind && existing.label == work.label)
            {
                if work.calls >= existing.calls && work.busy_nanos >= existing.busy_nanos {
                    *existing = work.clone();
                }
            } else if self.work.len() < 128 {
                self.work.push(work.clone());
            }
        }
        for protocol in &incoming.protocols {
            if !self
                .protocols
                .iter()
                .any(|existing| existing.boundary == protocol.boundary)
                && self.protocols.len() < 4
            {
                self.protocols.push(protocol.clone());
            }
        }
    }
}

/// Exchange-local measurement collector. Adapters can share it across worker
/// tasks without holding a lock over I/O or observer delivery.
#[derive(Clone, Debug)]
pub struct PerformanceRecorder {
    inner: Arc<RecorderInner>,
}
#[derive(Debug)]
struct RecorderInner {
    start: Instant,
    wall: SystemTime,
    evidence: Mutex<PerformanceEvidence>,
}
impl PerformanceRecorder {
    /// Anchors the monotonic timeline at complete incoming request headers.
    pub fn new(wall: SystemTime, start: Instant) -> Self {
        Self {
            inner: Arc::new(RecorderInner {
                start,
                wall,
                evidence: Mutex::new(PerformanceEvidence::default()),
            }),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, PerformanceEvidence> {
        self.inner
            .evidence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Records the first occurrence of a local milestone.
    pub fn mark(&self, milestone: Milestone) {
        self.mark_at(milestone, Instant::now());
    }
    /// Records an earlier physical connection milestone on this same clock.
    pub fn mark_at(&self, milestone: Milestone, instant: Instant) {
        let before = instant < self.inner.start;
        let duration = if before {
            self.inner.start.duration_since(instant)
        } else {
            instant.duration_since(self.inner.start)
        };
        let micros = i64::try_from(duration.as_micros()).unwrap_or(i64::MAX);
        let wall = if before {
            self.inner.wall.checked_sub(duration)
        } else {
            self.inner.wall.checked_add(duration)
        };
        let point = TimingPoint {
            milestone,
            unix_millis: wall
                .and_then(|wall| wall.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| {
                    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
                }),
            offset_micros: if before { -micros } else { micros },
        };
        let mut evidence = self.lock();
        if !evidence
            .points
            .iter()
            .any(|point| point.milestone == milestone)
        {
            evidence.points.push(point);
        }
    }
    /// Projects a physical monotonic instant onto this request's zero timestamp.
    pub fn offset_micros(&self, instant: Instant) -> i64 {
        if instant < self.inner.start {
            -i64::try_from(self.inner.start.duration_since(instant).as_micros()).unwrap_or(i64::MAX)
        } else {
            i64::try_from(instant.duration_since(self.inner.start).as_micros()).unwrap_or(i64::MAX)
        }
    }
    /// Keeps original setup ages while charging only this request's overlap.
    pub fn project_connection_setup(
        &self,
        observation: &mut TransportObservation,
        timings: &[ConnectionSetupTime],
    ) {
        let admitted = self
            .lock()
            .points
            .iter()
            .find(|point| point.milestone == Milestone::UpstreamBegin)
            .map_or(0, |point| point.offset_micros);
        observation.setup_timings = timings
            .iter()
            .take(4)
            .map(|timing| {
                let began = self.offset_micros(timing.began);
                let ended = self.offset_micros(timing.ended);
                let wait = ended
                    .saturating_sub(began.max(admitted).max(0))
                    .max(0)
                    .cast_unsigned();
                match timing.phase {
                    "dns" => observation.dns_micros = Some(wait),
                    "tcp" => observation.tcp_micros = Some(wait),
                    "tls" | "quic" => observation.tls_micros = Some(wait),
                    _ => {}
                }
                ConnectionTiming {
                    phase: timing.phase.into(),
                    began_offset_micros: began,
                    ended_offset_micros: ended,
                    request_wait_micros: wait,
                }
            })
            .collect();
    }

    /// Starts one measured operation, storing a bounded friendly label.
    pub fn work(&self, kind: &'static str, label: &str) -> WorkTimer {
        WorkTimer {
            recorder: self.clone(),
            kind,
            label: label.chars().take(128).collect(),
            began: Instant::now(),
        }
    }
    /// Records an explicitly observed operation interval, including zero waits.
    pub fn record_work(&self, kind: &str, label: &str, began: Instant, ended: Instant) {
        if ended < began {
            return;
        }
        let label = label.chars().take(128).collect::<String>();
        let kind = kind.chars().take(64).collect::<String>();
        let start = self.offset_micros(began);
        let end = self.offset_micros(ended);
        let busy =
            u64::try_from(ended.saturating_duration_since(began).as_nanos()).unwrap_or(u64::MAX);
        let mut evidence = self.lock();
        let overflow = evidence.work.len() >= 127
            && !evidence
                .work
                .iter()
                .any(|row| row.kind == kind && row.label == label);
        let (kind, label) = if overflow {
            ("other".into(), "Additional measured work".into())
        } else {
            (kind, label)
        };
        if let Some(row) = evidence
            .work
            .iter_mut()
            .find(|row| row.kind == kind && row.label == label)
        {
            row.began_offset_micros = row.began_offset_micros.min(start);
            row.ended_offset_micros = row.ended_offset_micros.max(end);
            row.busy_nanos = row.busy_nanos.saturating_add(busy);
            row.calls = row.calls.saturating_add(1);
        } else if evidence.work.len() < 128 {
            evidence.work.push(WorkTiming {
                kind,
                label,
                began_offset_micros: start,
                ended_offset_micros: end,
                busy_nanos: busy,
                calls: 1,
            });
        }
    }

    /// Updates the snapshot of a physical connection rather than charging its
    /// setup or byte counters to each stream independently.
    pub fn transport(&self, mut observation: TransportObservation) {
        observation.sampled_offset_micros =
            i64::try_from(self.inner.start.elapsed().as_micros()).unwrap_or(i64::MAX);
        let mut evidence = self.lock();
        if let Some(existing) = evidence.transports.iter_mut().find(|existing| {
            existing.leg == observation.leg && existing.connection_id == observation.connection_id
        }) {
            *existing = observation;
        } else if evidence.transports.len() < 16 {
            evidence.transports.push(observation);
        }
    }
    /// Updates one actual message boundary protocol.
    pub fn protocol(&self, observation: ProtocolObservation) {
        let mut evidence = self.lock();
        if let Some(existing) = evidence
            .protocols
            .iter_mut()
            .find(|existing| existing.boundary == observation.boundary)
        {
            *existing = observation;
        } else if evidence.protocols.len() < 4 {
            evidence.protocols.push(observation);
        }
    }
    /// Copies a bounded snapshot for observer delivery outside the lock.
    pub fn snapshot(&self) -> PerformanceEvidence {
        self.lock().clone()
    }
    /// Successful or failed terminal processing has been measured.
    pub fn finished(&self) -> bool {
        self.lock()
            .points
            .iter()
            .any(|point| point.milestone == Milestone::ExchangeDone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_connection_times_and_submillisecond_duration_use_one_clock() {
        let start = Instant::now();
        let recorder = PerformanceRecorder::new(
            UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000),
            start,
        );
        recorder.mark_at(
            Milestone::ClientConnected,
            start
                .checked_sub(std::time::Duration::from_secs(2))
                .unwrap(),
        );
        recorder.mark_at(Milestone::RequestHeaders, start);
        recorder.mark_at(
            Milestone::ExchangeDone,
            start + std::time::Duration::from_micros(550),
        );
        recorder.mark_at(
            Milestone::ExchangeDone,
            start + std::time::Duration::from_secs(1),
        );
        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.points[0].offset_micros, -2_000_000);
        assert_eq!(snapshot.points[0].unix_millis, 1_799_999_998_000);
        assert_eq!(snapshot.elapsed_micros(), Some(550));
        assert!(snapshot.valid());
        assert!(recorder.finished());
    }
    #[test]
    fn work_aggregates_actual_calls_rejects_stale_samples_and_bounds_names() {
        let zero = Instant::now();
        let recorder = PerformanceRecorder::new(SystemTime::now(), zero);
        recorder.record_work(
            "transform",
            "Request body · script",
            zero,
            zero + std::time::Duration::from_nanos(500),
        );
        let old = recorder.snapshot();
        recorder.record_work(
            "transform",
            "Request body · script",
            zero + std::time::Duration::from_secs(1),
            zero + std::time::Duration::from_secs(1) + std::time::Duration::from_nanos(600),
        );
        let mut latest = recorder.snapshot();
        assert_eq!(latest.work[0].busy_nanos, 1100);
        assert_eq!(latest.work[0].calls, 2);
        latest.merge(&old);
        assert_eq!(latest.work[0].busy_nanos, 1100);
        for index in 0..200 {
            recorder.record_work("hook", &format!("Hook {index}"), zero, zero);
        }
        let latest = recorder.snapshot();
        assert_eq!(latest.work.len(), 128);
        assert!(
            latest
                .work
                .iter()
                .any(|row| row.kind == "other" && row.calls > 1)
        );
        assert!(latest.valid());
        let timer = recorder.work("pause", "Dropped operation");
        drop(timer);
        assert!(recorder.snapshot().valid());
        assert_eq!(
            serde_json::from_slice::<PerformanceEvidence>(&serde_json::to_vec(&latest).unwrap())
                .unwrap(),
            latest
        );
    }

    #[test]
    fn shared_setup_preserves_its_age_and_charges_only_the_request_wait() {
        let zero = Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap();
        let recorder = PerformanceRecorder::new(SystemTime::now(), zero);
        recorder.mark_at(
            Milestone::UpstreamBegin,
            zero + std::time::Duration::from_millis(1),
        );
        let before = |micros| {
            zero.checked_sub(std::time::Duration::from_micros(micros))
                .unwrap()
        };
        let after = |micros| zero + std::time::Duration::from_micros(micros);
        let mut observation = TransportObservation {
            leg: "upstream".into(),
            connection_id: "pending-shared-h2".into(),
            shared: true,
            ..TransportObservation::default()
        };
        recorder.project_connection_setup(
            &mut observation,
            &[
                ConnectionSetupTime {
                    phase: "dns",
                    began: before(30_000),
                    ended: before(20_000),
                },
                ConnectionSetupTime {
                    phase: "tcp",
                    began: before(20_000),
                    ended: before(10_000),
                },
                ConnectionSetupTime {
                    phase: "tls",
                    began: before(10_000),
                    ended: after(5_000),
                },
            ],
        );
        assert_eq!(observation.dns_micros, Some(0));
        assert_eq!(observation.tcp_micros, Some(0));
        assert_eq!(observation.tls_micros, Some(4_000));
        assert_eq!(observation.setup_timings[2].began_offset_micros, -10_000);
        assert_eq!(observation.setup_timings[2].ended_offset_micros, 5_000);
        assert_eq!(observation.setup_timings[2].request_wait_micros, 4_000);
        recorder.transport(observation);
        let snapshot = recorder.snapshot();
        assert!(snapshot.valid());
        assert_eq!(
            serde_json::from_slice::<PerformanceEvidence>(&serde_json::to_vec(&snapshot).unwrap())
                .unwrap(),
            snapshot
        );
        let reused = PerformanceRecorder::new(SystemTime::now(), after(30_000));
        let mut connection = TransportObservation::default();
        reused.project_connection_setup(
            &mut connection,
            &[ConnectionSetupTime {
                phase: "quic",
                began: before(10_000),
                ended: after(5_000),
            }],
        );
        assert_eq!(connection.tls_micros, Some(0));
        assert_eq!(connection.setup_timings[0].began_offset_micros, -40_000);
        assert_eq!(connection.setup_timings[0].ended_offset_micros, -25_000);
    }

    #[test]
    fn delayed_samples_cannot_erase_terminal_points_or_newer_shared_counters() {
        let mut latest = PerformanceEvidence {
            points: vec![TimingPoint {
                milestone: Milestone::ExchangeDone,
                unix_millis: 12,
                offset_micros: 10,
            }],
            transports: vec![TransportObservation {
                leg: "upstream".into(),
                connection_id: "pool-1".into(),
                sampled_offset_micros: 20,
                bytes_read: Some(999),
                shared: true,
                ..TransportObservation::default()
            }],
            protocols: vec![],
            work: vec![],
        };
        let mut stale = latest.clone();
        stale.points.clear();
        stale.transports[0].sampled_offset_micros = 1;
        stale.transports[0].bytes_read = Some(1);
        stale.transports[0].shared = false;
        latest.merge(&stale);
        assert_eq!(latest.transports[0].bytes_read, Some(999));
        assert!(latest.transports[0].shared);
        assert_eq!(latest.points.len(), 1);
        stale.points = vec![latest.points[0].clone(), latest.points[0].clone()];
        assert!(!stale.valid());
    }
}
