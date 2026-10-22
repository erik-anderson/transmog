//! Incremental SSE observation over arbitrary transfer chunks.
use crate::{ContentCodingStack, ContentDecoder, ContentLimits};
use bytes::Bytes;
use std::num::NonZeroUsize;
use transmog_core::{BodyFrame, HeaderBlock, stream::StreamMessage};

pub use transmog_core::stream::is_event_stream;

/// Passive SSE decoder; a decoding failure does not affect forwarded HTTP bytes.
pub struct SseDecoder {
    codecs: Vec<ContentDecoder>,
    parser: EventParser,
}
impl SseDecoder {
    /// Creates independent decoding state for a response.
    ///
    /// # Errors
    /// Returns an explanation for unsupported content encodings.
    pub fn new(headers: &HeaderBlock) -> Result<Self, String> {
        let limits = ContentLimits::new(
            NonZeroUsize::MAX,
            NonZeroUsize::MAX,
            NonZeroUsize::MAX,
            const { NonZeroUsize::new(8 * 1024 * 1024).unwrap() },
            const { NonZeroUsize::new(100).unwrap() },
            1024 * 1024,
            const { NonZeroUsize::new(4).unwrap() },
        );
        let limits = limits.for_event_stream();
        let stack = ContentCodingStack::from_headers(headers, limits.max_coding_layers())
            .map_err(|e| e.to_string())?;
        let codecs = stack
            .decode_order()
            .map(|coding| ContentDecoder::new(coding, limits).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            codecs,
            parser: EventParser::default(),
        })
    }

    /// Decodes newly received bytes without waiting for the end of the response.
    ///
    /// # Errors
    /// Returns a content-decoding error; callers should mark inspection unavailable.
    pub async fn push(&mut self, frame: BodyFrame) -> Result<Vec<StreamMessage>, String> {
        let mut frames = vec![frame];
        for codec in &mut self.codecs {
            let mut next = Vec::new();
            for frame in frames {
                next.extend(codec.on_frame(frame).await.map_err(|e| e.to_string())?);
            }
            frames = next;
        }
        Ok(self.parse(frames))
    }

    /// Validates coding trailers and exposes incomplete final event data explicitly.
    ///
    /// # Errors
    /// Returns invalid or incomplete compressed-content errors.
    pub async fn finish(&mut self) -> Result<Vec<StreamMessage>, String> {
        let mut frames = Vec::new();
        for codec in &mut self.codecs {
            let mut next = Vec::new();
            for frame in frames {
                next.extend(codec.on_frame(frame).await.map_err(|e| e.to_string())?);
            }
            next.extend(codec.finish().await.map_err(|e| e.to_string())?);
            frames = next;
        }
        let mut messages = self.parse(frames);
        self.parser.finish(&mut messages);
        Ok(messages)
    }

    fn parse(&mut self, frames: Vec<BodyFrame>) -> Vec<StreamMessage> {
        let mut messages = Vec::new();
        for frame in frames {
            if let BodyFrame::Data(bytes) = frame {
                self.parser.push(&bytes, &mut messages);
            }
        }
        messages
    }
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // Independent SSE data/comment, BOM and CR parser flags.
struct EventParser {
    line: Vec<u8>,
    raw: Vec<u8>,
    data: String,
    event: String,
    id: std::sync::Arc<str>,
    retry: Option<u64>,
    fields: usize,
    has_data: bool,
    comment: bool,
    started: bool,
    cr: bool,
}
impl EventParser {
    fn push(&mut self, bytes: &[u8], output: &mut Vec<StreamMessage>) {
        for &byte in bytes {
            if self.cr {
                self.cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.raw.push(b'\n');
                    self.cr = true;
                    self.end_line(output);
                }
                b'\n' => {
                    self.raw.push(byte);
                    self.end_line(output);
                }
                _ => {
                    self.raw.push(byte);
                    self.line.push(byte);
                }
            }
        }
    }
    fn end_line(&mut self, output: &mut Vec<StreamMessage>) {
        let bytes = std::mem::take(&mut self.line);
        let mut line = String::from_utf8_lossy(&bytes);
        if !self.started {
            self.started = true;
            line = line
                .strip_prefix('\u{feff}')
                .unwrap_or(&line)
                .to_owned()
                .into();
        }
        if line.is_empty() {
            if self.has_data || self.comment || self.fields > 0 {
                let mut message = StreamMessage::new(
                    "server",
                    if self.has_data {
                        "event"
                    } else if self.comment {
                        "heartbeat"
                    } else {
                        "fields"
                    },
                    Bytes::from(
                        self.data
                            .strip_suffix('\n')
                            .unwrap_or(&self.data)
                            .to_owned(),
                    ),
                );
                message.event = self.has_data.then(|| {
                    if self.event.is_empty() {
                        "message".into()
                    } else {
                        self.event.clone()
                    }
                });
                message.event_id = Some(self.id.clone());
                message.retry = self.retry;
                message.raw = Some(String::from_utf8_lossy(&self.raw).into_owned());
                output.push(message);
            }
            self.raw.clear();
            self.data.clear();
            self.event.clear();
            self.has_data = false;
            self.comment = false;
            self.fields = 0;
            return;
        }
        if line.starts_with(':') {
            self.comment = true;
            return;
        }
        let (name, value) = line.split_once(':').unwrap_or((&line, ""));
        self.fields += 1;
        let value = value.strip_prefix(' ').unwrap_or(value);
        match name {
            "data" => {
                self.has_data = true;
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => value.clone_into(&mut self.event),
            "id" if !value.contains('\0') => self.id = value.into(),
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                if let Ok(retry) = value.parse() {
                    self.retry = Some(retry);
                }
            }
            _ => {}
        }
    }
    fn finish(&mut self, output: &mut Vec<StreamMessage>) {
        if !self.raw.is_empty() {
            output.push(StreamMessage::new(
                "server",
                "incomplete",
                Bytes::from(std::mem::take(&mut self.raw)),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn large_events_are_complete_and_persistent_ids_share_storage() {
        let mut decoder = SseDecoder::new(&HeaderBlock::default()).unwrap();
        let data = "x".repeat(1024 * 1024);
        let initial = format!("id: {}\ndata: {data}\n\n", "i".repeat(64 * 1024));
        let first = decoder
            .push(BodyFrame::Data(Bytes::from(initial)))
            .await
            .unwrap();
        assert_eq!(first[0].payload.len(), data.len());
        assert!(!first[0].truncated);
        let messages = decoder
            .push(BodyFrame::Data(Bytes::from(b":\n\n".repeat(256))))
            .await
            .unwrap();
        assert_eq!(messages.len(), 256);
        let id = first[0].event_id.as_ref().unwrap();
        assert!(
            messages
                .iter()
                .all(|message| std::sync::Arc::ptr_eq(id, message.event_id.as_ref().unwrap()))
        );
    }
    #[test]
    fn every_chunk_boundary_preserves_sse_fields_and_raw_text() {
        let wire = "\u{feff}id: 12\r\nevent: update\rdata: héllo\ndata: world\nretry: 4500\n\n: keepalive\n\ndata: tail";
        for split in 0..=wire.len() {
            let mut parser = EventParser::default();
            let mut messages = Vec::new();
            parser.push(&wire.as_bytes()[..split], &mut messages);
            parser.push(&wire.as_bytes()[split..], &mut messages);
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].payload, "héllo\nworld");
            assert_eq!(messages[0].event.as_deref(), Some("update"));
            assert_eq!(messages[0].event_id.as_deref(), Some("12"));
            assert_eq!(messages[0].retry, Some(4500));
            assert_eq!(messages[1].kind, "heartbeat");
            parser.finish(&mut messages);
            assert_eq!(messages[2].kind, "incomplete");
        }
    }
    #[tokio::test]
    async fn gzip_event_stream_decodes_across_single_byte_boundaries() {
        let mut encoder =
            crate::ContentEncoder::new(crate::ContentCoding::Gzip, ContentLimits::default())
                .unwrap();
        let mut wire = encoder
            .on_frame(BodyFrame::Data(Bytes::from_static(b"data: compressed\r\r")))
            .await
            .unwrap();
        wire.extend(encoder.finish().await.unwrap());
        let headers = HeaderBlock::from_fields(vec![
            transmog_core::HeaderField::try_new("Content-Encoding", "gzip").unwrap(),
        ]);
        let mut decoder = SseDecoder::new(&headers).unwrap();
        let mut messages = Vec::new();
        for frame in wire {
            if let BodyFrame::Data(bytes) = frame {
                for byte in bytes {
                    messages.extend(
                        decoder
                            .push(BodyFrame::Data(Bytes::from(vec![byte])))
                            .await
                            .unwrap(),
                    );
                }
            }
        }
        assert_eq!(messages.len(), 1, "events must arrive before finish");
        assert_eq!(messages[0].payload, "compressed");
        assert_eq!(
            decoder.finish().await.unwrap(),
            [] as [transmog_core::stream::StreamMessage; 0]
        );
    }
    #[test]
    fn carriage_return_dispatches_without_waiting_for_another_chunk() {
        let mut parser = EventParser::default();
        let mut messages = Vec::new();
        parser.push(b"data: live\r\r", &mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload, "live");
    }

    #[test]
    fn id_reset_empty_data_and_invalid_retry_follow_event_stream_rules() {
        let mut parser = EventParser::default();
        let mut messages = Vec::new();
        parser.push(
            b"id: first\n\ndata:\n\nid:\nretry: -3\ndata: next\n\n",
            &mut messages,
        );
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].kind, "fields");
        assert_eq!(messages[0].event_id.as_deref(), Some("first"));
        assert!(messages[1].payload.is_empty());
        assert_eq!(messages[2].event_id.as_deref(), Some(""));
        assert_eq!(messages[2].retry, None);
    }
}
