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
    /// System DNS resolution duration, absent when no lookup occurred.
    pub dns_micros: Option<u64>,
    /// TCP connect race duration, separate from DNS and TLS.
    pub tcp_micros: Option<u64>,
    /// TLS handshake duration, absent for plaintext or unavailable adapters.
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
    /// QUIC recovery/path statistics; absent on TCP adapters.
    #[serde(default)]
    pub quic: Option<QuicObservation>,
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
            && self.points.iter().enumerate().all(|(index, point)| {
                point.unix_millis <= 253_402_300_799_999
                    && !self.points[..index]
                        .iter()
                        .any(|other| other.milestone == point.milestone)
            })
            && self.transports.iter().all(|transport| {
                matches!(transport.leg.as_str(), "client" | "upstream")
                    && transport.connection_id.len() <= 128
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
