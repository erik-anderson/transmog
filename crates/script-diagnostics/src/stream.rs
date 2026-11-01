//! Bounded native writer queues with capacity reserved for control messages.
//! A single writer owns framing; logging never writes to stdout directly.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
type WriteReply<T> = Box<dyn FnMut(&T) -> bool + Send>;
enum OutputState<T> {
    Direct(WriteReply<T>),
    Queued(EventSender<T>, std::thread::JoinHandle<bool>),
    Closed,
}
/// Native output owner. Ordinary results stay synchronous until logging or
/// debugging explicitly needs a bounded asynchronous writer.
pub struct Output<T>(Arc<Mutex<OutputState<T>>>);
/// Cloneable event producer; every producer shares the same ordered writer.
pub struct OutputSender<T>(Arc<Mutex<OutputState<T>>>);
impl<T> Drop for Output<T> {
    fn drop(&mut self) {
        // Also close producers during unwinding; only explicit finish waits for I/O.
        let previous = std::mem::replace(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            OutputState::Closed,
        );
        drop(previous);
    }
}
impl<T> Clone for OutputSender<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<T: Send + 'static> Output<T> {
    /// Creates a direct writer without a thread or queue. The callback must return
    /// false on serialization/transport failure and must never execute script code.
    pub fn new(write: impl FnMut(&T) -> bool + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(OutputState::Direct(Box::new(write)))))
    }
    /// Shares native submission with an engine or authenticated input router.
    pub fn sender(&self) -> OutputSender<T> {
        OutputSender(Arc::clone(&self.0))
    }
    /// Closes all producers and joins the optional writer. Returns false on failure.
    pub fn finish(self) -> bool {
        let previous = std::mem::replace(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            OutputState::Closed,
        );
        match previous {
            OutputState::Direct(_) => true,
            OutputState::Closed => false,
            OutputState::Queued(sender, thread) => {
                drop(sender);
                thread.join().unwrap_or(false)
            }
        }
    }
}
impl<T: Send + 'static> OutputSender<T> {
    fn queue(state: &mut OutputState<T>) -> Option<&EventSender<T>> {
        if matches!(state, OutputState::Direct(_)) {
            let OutputState::Direct(mut write) = std::mem::replace(state, OutputState::Closed)
            else {
                unreachable!()
            };
            let (sender, receiver) = channel();
            let thread = std::thread::Builder::new()
                .name("script-event-writer".into())
                .spawn(move || {
                    while let Some(reply) = receiver.recv() {
                        if !write(&reply) {
                            return false;
                        }
                    }
                    true
                })
                .ok()?;
            *state = OutputState::Queued(sender, thread);
        }
        if let OutputState::Queued(sender, _) = state {
            Some(sender)
        } else {
            None
        }
    }
    /// Sends a result directly, or behind previously accepted events after activation.
    pub fn control(&self, value: T) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            OutputState::Direct(write) => {
                if write(&value) {
                    true
                } else {
                    *state = OutputState::Closed;
                    false
                }
            }
            OutputState::Queued(sender, _) => sender.control(value),
            OutputState::Closed => false,
        }
    }
    /// Activates the bounded writer and attempts one lossy console event.
    pub fn log(&self, value: T) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::queue(&mut state).is_some_and(|sender| sender.log(value))
    }
    /// Activates reserved control capacity for debugger events and stream endings.
    pub fn event(&self, value: T) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::queue(&mut state).is_some_and(|sender| sender.control(value))
    }
}
const LOG_DEPTH: usize = 128;
const CONTROL_DEPTH: usize = 32;
const LOG_FLAG: u32 = 1 << 31;
/// Maximum serialized console event; checked before incoming payload allocation.
pub const MAX_LOG_FRAME_BYTES: usize = 64 * 1024;

/// Reads a classed worker reply; large ordinary results do not raise the log ceiling.
/// # Errors
/// Rejects empty/excessive lengths before allocating and propagates transport failure.
pub fn read_payload(
    input: &mut impl std::io::Read,
    result_limit: usize,
) -> std::io::Result<(bool, Vec<u8>)> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix)?;
    let header = u32::from_le_bytes(prefix);
    let log = header & LOG_FLAG != 0;
    let length = (header & !LOG_FLAG) as usize;
    let limit = if log {
        MAX_LOG_FRAME_BYTES
    } else {
        result_limit
    };
    if length == 0 || length > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid worker reply extent",
        ));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok((log, bytes))
}

/// Writes one classed reply through its sole native writer.
/// # Errors
/// Rejects excessive output and propagates transport failure.
pub fn write_payload(
    output: &mut impl std::io::Write,
    payload: &[u8],
    log: bool,
    result_limit: usize,
) -> std::io::Result<()> {
    let limit = if log {
        MAX_LOG_FRAME_BYTES
    } else {
        result_limit
    };
    if payload.is_empty() || payload.len() > limit || payload.len() >= LOG_FLAG as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid worker reply extent",
        ));
    }
    let length = u32::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid worker reply extent",
        )
    })?;
    output.write_all(&(length | if log { LOG_FLAG } else { 0 }).to_le_bytes())?;
    output.write_all(payload)?;
    output.flush()
}

/// Producer for one owned worker's output stream.
pub struct EventSender<T> {
    sender: SyncSender<(bool, T)>,
    logs: Arc<AtomicUsize>,
}
impl<T> Clone for EventSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            logs: Arc::clone(&self.logs),
        }
    }
}
impl<T> EventSender<T> {
    /// Enqueues a lossy event without waiting for a stalled reader.
    pub fn log(&self, value: T) -> bool {
        if self
            .logs
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < LOG_DEPTH).then_some(count + 1)
            })
            .is_err()
        {
            return false;
        }
        if self.sender.try_send((true, value)).is_err() {
            self.logs.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }
    /// Enqueues an ordered result/state message using reserved capacity.
    /// Failure means the bounded transport is closed or protocol work has flooded it.
    pub fn control(&self, value: T) -> bool {
        self.sender.try_send((false, value)).is_ok()
    }
}

/// Single native output consumer. Dropping it closes every producer.
pub struct EventReceiver<T> {
    receiver: Receiver<(bool, T)>,
    logs: Arc<AtomicUsize>,
}
impl<T> EventReceiver<T> {
    /// Waits for the next ordered frame, or closure of all producers.
    pub fn recv(&self) -> Option<T> {
        let (log, value) = self.receiver.recv().ok()?;
        if log {
            self.logs.fetch_sub(1, Ordering::AcqRel);
        }
        Some(value)
    }
}
/// Creates finite log/control capacity. Control frames follow previously accepted
/// logs, so results form a delivery barrier; later logging cannot starve them.
pub fn channel<T>() -> (EventSender<T>, EventReceiver<T>) {
    let (sender, receiver) = mpsc::sync_channel(LOG_DEPTH + CONTROL_DEPTH);
    let logs = Arc::new(AtomicUsize::new(0));
    (
        EventSender {
            sender,
            logs: Arc::clone(&logs),
        },
        EventReceiver { receiver, logs },
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn silent_output_stays_direct_and_activation_preserves_a_single_writer_order() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let values = Arc::clone(&seen);
        let output = super::Output::new(move |value: &u32| {
            values.lock().unwrap().push(*value);
            true
        });
        let sender = output.sender();
        assert!(sender.control(1));
        assert!(matches!(
            *output.0.lock().unwrap(),
            super::OutputState::Direct(_)
        ));
        assert!(sender.log(2));
        assert!(sender.control(3));
        assert!(sender.event(4));
        assert!(output.finish());
        assert_eq!(*seen.lock().unwrap(), vec![1, 2, 3, 4]);
        assert!(!sender.control(5));
    }
    #[test]
    fn console_frame_limits_apply_before_reading_or_allocating_the_payload() {
        let prefix = (super::LOG_FLAG | 0x0001_0001).to_le_bytes();
        let error = super::read_payload(&mut prefix.as_slice(), 68 * 1024 * 1024).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        let mut wire = Vec::new();
        super::write_payload(&mut wire, b"console", true, 68 * 1024 * 1024).unwrap();
        assert_eq!(
            super::read_payload(&mut wire.as_slice(), 68 * 1024 * 1024).unwrap(),
            (true, b"console".to_vec())
        );
    }
    #[test]
    fn a_stalled_reader_drops_logs_and_preserves_ordered_control_capacity() {
        let (sender, receiver) = super::channel();
        for number in 0..128 {
            assert!(sender.log(number));
        }
        assert!(!sender.log(128));
        assert!(sender.control(999));
        for number in 0..128 {
            assert_eq!(receiver.recv(), Some(number));
        }
        assert_eq!(receiver.recv(), Some(999));
        drop(sender);
        assert_eq!(receiver.recv(), None);
    }
}
