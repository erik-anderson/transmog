//! MCP diagnostics must never block protocol handling or process shutdown.
use std::{
    fmt::Display,
    io::{self, Write as _},
    sync::mpsc::{self, SyncSender},
    time::Duration,
};
use tracing_subscriber::fmt::MakeWriter;

const MAX_RECORD_BYTES: usize = 8 * 1024;
const QUEUED_RECORDS: usize = 64;

enum Message {
    Record(Vec<u8>),
    Finish(mpsc::Sender<()>),
}

#[derive(Clone)]
pub(crate) struct Diagnostics {
    sender: SyncSender<Message>,
}

impl Diagnostics {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(QUEUED_RECORDS);
        // Dropping this handle detaches the worker. Even an OS write blocked on
        // an unread pipe cannot keep main alive. If thread creation fails, the
        // disconnected queue drops diagnostics rather than blocking the caller.
        let _ = std::thread::Builder::new()
            .name("mcp-diagnostics".into())
            .spawn(move || {
                let mut stderr = io::stderr();
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Record(record) => {
                            if stderr.write_all(&record).is_err() {
                                break;
                            }
                        }
                        Message::Finish(done) => {
                            let _ = done.send(());
                            break;
                        }
                    }
                }
            });
        Self { sender }
    }

    pub(crate) fn error(&self, error: &dyn Display) {
        // Required command errors remain independent of the RUST_LOG filter.
        let _ = writeln!(self.make_writer(), "transmog-cli: {error}");
    }

    pub(crate) fn finish(&self) {
        let (done, finished) = mpsc::channel();
        if self.sender.try_send(Message::Finish(done)).is_ok() {
            // Best effort delivery; an unresponsive stderr reader cannot turn
            // diagnostic flushing into another unbounded shutdown wait.
            let _ = finished.recv_timeout(Duration::from_millis(100));
        }
    }
}

impl<'a> MakeWriter<'a> for Diagnostics {
    type Writer = RecordWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RecordWriter {
            sender: self.sender.clone(),
            bytes: Vec::new(),
            oversized: false,
        }
    }
}

pub(crate) struct RecordWriter {
    sender: SyncSender<Message>,
    bytes: Vec<u8>,
    oversized: bool,
}

impl io::Write for RecordWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.oversized {
            if bytes.len() > MAX_RECORD_BYTES - self.bytes.len() {
                self.oversized = true;
                self.bytes.clear();
            } else {
                self.bytes.extend_from_slice(bytes);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for RecordWriter {
    fn drop(&mut self) {
        if !self.oversized && !self.bytes.is_empty() {
            let _ = self
                .sender
                .try_send(Message::Record(std::mem::take(&mut self.bytes)));
        }
    }
}
