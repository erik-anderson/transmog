//! CLI circular observer: newest exchange groups, no implicit memory-mode journal.
use std::{
    collections::HashMap,
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use transmog_capture::{
    CaptureBodyRetention, CaptureEncoding, CaptureLimits, CapturePolicy, CaptureRecordKind,
    CircularCapture, loss_record, record_from_observer,
};
use transmog_core::observe::{
    BodyObservation, BoxObserverFuture, ObservationInterest, Observer, ObserverConfig,
    ObserverDeliveryPolicy, ObserverEvent,
};

pub(crate) struct CircularObserver {
    state: Arc<Mutex<State>>,
    policy: CapturePolicy,
    redact: bool,
    _directory: Option<tempfile::TempDir>,
}
struct State {
    ring: CircularCapture,
    retention: CaptureBodyRetention,
    sequences: HashMap<u128, u64>,
    failure: Option<String>,
}
impl CircularObserver {
    pub(crate) fn new(
        value: &str,
        root: PathBuf,
        encoding: CaptureEncoding,
        policy: CapturePolicy,
        redact: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        let half = if system.total_memory() > 1 {
            system.total_memory() / 2
        } else {
            1_073_741_824
        };
        let maximum = if value == "unlimited" {
            None
        } else if value == "auto" {
            Some(half)
        } else {
            Some(parse_size(value)?)
        };
        let disk = maximum.is_none_or(|bytes| bytes > half);
        let directory = if disk {
            std::fs::create_dir_all(&root)?;
            Some(tempfile::tempdir_in(root)?)
        } else {
            None
        };
        println!(
            "Circular capture: {} · {}. Only newest retained exchanges are saved when stopped.",
            maximum.map_or_else(
                || "no maximum".into(),
                |bytes| format!("{bytes} bytes maximum")
            ),
            if disk {
                "disk storage"
            } else {
                "memory storage; an interrupted process loses its unsaved buffer"
            }
        );
        let ring = CircularCapture::new(
            maximum,
            directory.as_ref().map(|dir| dir.path().into()),
            encoding,
            CaptureLimits::default(),
        )?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                ring,
                retention: CaptureBodyRetention::default(),
                sequences: HashMap::new(),
                failure: None,
            })),
            policy,
            redact,
            _directory: directory,
        })
    }
    pub(crate) fn metadata(
        &self,
        metadata: serde_json::Value,
    ) -> Result<(), transmog_capture::CaptureError> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ring
            .append(&transmog_capture::CaptureRecord {
                exchange_id: 0,
                sequence: 0,
                kind: CaptureRecordKind::Unknown {
                    kind: "trace-metadata".into(),
                    payload: metadata,
                },
            })
    }
    pub(crate) fn config(&self) -> ObserverConfig {
        ObserverConfig {
            interest: ObservationInterest {
                lifecycle: true,
                sensitive_headers: !self.redact,
                request_body: BodyObservation::Full,
                response_body: BodyObservation::Full,
            },
            queue_capacity: NonZeroUsize::new(2048).unwrap_or(NonZeroUsize::MIN),
            delivery: ObserverDeliveryPolicy::Backpressure {
                timeout: Duration::from_millis(25),
            },
            callback_timeout: Duration::from_secs(10),
        }
    }
    pub(crate) fn check(&self) -> io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure
            .as_ref()
            .map_or(Ok(()), |message| Err(io::Error::other(message.clone())))
    }
    pub(crate) fn save(&self, path: &std::path::Path) -> io::Result<()> {
        self.check()?;
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = state
            .ring
            .write(&file)
            .map_err(io::Error::other)
            .and_then(|bytes| {
                file.sync_all()?;
                println!(
                    "Circular trace saved: {bytes} bytes; {} older exchanges evicted.",
                    state.ring.evicted_exchanges()
                );
                Ok(())
            });
        // A partial encrypted native file remains recoverable if the final write failed.
        result
    }
}
impl Observer for CircularObserver {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        let state = self.state.clone();
        let policy = self.policy.clone();
        Box::pin(async move {
            let worker = state.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut state = worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let id = event.exchange_id.0;
                if !state.ring.retains_exchange(id)
                    && !matches!(
                        event.kind,
                        transmog_core::observe::ObserverEventKind::ExchangeStarted { .. }
                    )
                {
                    return Ok::<(), transmog_capture::CaptureError>(());
                }
                let previous = state.sequences.get(&id).copied().unwrap_or(0);
                if event.sequence <= previous {
                    return Err(transmog_capture::CaptureError::MalformedRecord(
                        "Observer sequence moved backward",
                    ));
                }
                if event.sequence > previous.saturating_add(1) {
                    state.ring.append(&loss_record(
                        id,
                        previous + 1,
                        event.sequence - previous - 1,
                        "observer-delivery-gap",
                    ))?;
                }
                let evicted = state.ring.evicted_exchanges();
                if let Some(mut record) = record_from_observer(&event, &policy) {
                    state.retention.apply(&mut record, &policy);
                    state.ring.append(&record)?;
                }
                state.sequences.insert(id, event.sequence);
                if state.ring.evicted_exchanges() != evicted {
                    let State {
                        ring, sequences, ..
                    } = &mut *state;
                    sequences.retain(|id, _| ring.retains_exchange(*id));
                }
                Ok(())
            })
            .await;
            if let Err(message) = result
                .map_err(|_| "Circular capture worker failed".to_owned())
                .and_then(|result| result.map_err(|error| error.to_string()))
            {
                state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .failure = Some(message);
            }
            Ok(())
        })
    }
}
pub(crate) fn parse_size(value: &str) -> io::Result<u64> {
    let split = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    let amount = value[..split].parse::<u64>().map_err(|_| {
        crate::invalid_input("Circular buffer size must be a positive byte count or auto/unlimited")
    })?;
    let unit = match value[split..].to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "mib" => 1_048_576,
        "gib" => 1_073_741_824,
        _ => {
            return Err(crate::invalid_input(
                "Use bytes, MB, GB, MiB or GiB for circular buffer size",
            ));
        }
    };
    amount
        .checked_mul(unit)
        .filter(|bytes| *bytes >= 1_048_576)
        .ok_or_else(|| {
            crate::invalid_input(
                "Circular buffer must be at least 1 MiB and fit a 64-bit byte count",
            )
        })
}
