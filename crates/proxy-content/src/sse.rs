//! Incremental, bounded SSE observation over arbitrary transfer chunks.
use crate::{ContentCodingStack, ContentDecoder, ContentLimits};
use bytes::Bytes;
use std::num::NonZeroUsize;
use transmog_core::{
    BodyFrame, HeaderBlock,
    stream::{MESSAGE_BYTES, StreamMessage},
};

pub use transmog_core::stream::is_event_stream;

/// Passive SSE decoder; a decoding failure does not affect forwarded HTTP bytes.
pub struct SseDecoder {
    codecs: Vec<ContentDecoder>,
    parser: EventParser,
}
impl SseDecoder {
    /// Creates independent bounded decoding state for a response.
    ///
    /// # Errors
    /// Returns an explanation for unsupported content encodings.
    pub fn new(headers: &HeaderBlock) -> Result<Self, String> {
        let limits = ContentLimits::new(
            NonZeroUsize::MAX,
            NonZeroUsize::MAX,
            NonZeroUsize::MAX,
            NonZeroUsize::new(8 * 1024 * 1024).unwrap(),
            NonZeroUsize::new(100).unwrap(),
            1024 * 1024,
            NonZeroUsize::new(4).unwrap(),
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
    /// Returns a decoding or event-size error; callers should mark inspection unavailable.
    pub async fn push(&mut self, frame: BodyFrame) -> Result<Vec<StreamMessage>, String> {
        let mut frames = vec![frame];
        for codec in &mut self.codecs {
            let mut next = Vec::new();
            for frame in frames {
                next.extend(codec.on_frame(frame).await.map_err(|e| e.to_string())?);
            }
            frames = next;
        }
        self.parse(frames)
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
        let mut messages = self.parse(frames)?;
        self.parser.finish(&mut messages);
        Ok(messages)
    }

    fn parse(&mut self, frames: Vec<BodyFrame>) -> Result<Vec<StreamMessage>, String> {
        let mut messages = Vec::new();
        for frame in frames {
            if let BodyFrame::Data(bytes) = frame {
                self.parser.push(&bytes, &mut messages)?;
            }
        }
        Ok(messages)
    }
}

#[derive(Default)]
struct EventParser {
    line: Vec<u8>,
    raw: Vec<u8>,
    data: String,
    event: String,
    id: String,
    retry: Option<u64>,
    has_data: bool,
    comment: bool,
    first_line: bool,
    started: bool,
    cr: bool,
}
impl EventParser {
    fn push(&mut self, bytes: &[u8], output: &mut Vec<StreamMessage>) -> Result<(), String> {
        for &byte in bytes {
            if self.cr {
                self.cr = false;
                if byte == b'\n' {
                    self.raw.push(byte);
                    self.end_line(output);
                    continue;
                }
                self.end_line(output);
            }
            if self.raw.len() >= MESSAGE_BYTES {
                return Err(
                    "SSE event exceeds the 256 KiB inspection limit; forwarding continues".into(),
                );
            }
            self.raw.push(byte);
            match byte {
                b'\r' => self.cr = true,
                b'\n' => self.end_line(output),
                _ => self.line.push(byte),
            }
        }
        Ok(())
    }
    fn end_line(&mut self, output: &mut Vec<StreamMessage>) {
        let bytes = std::mem::take(&mut self.line);
        let mut line = String::from_utf8_lossy(&bytes);
        if !self.started {
            self.started = true;
            self.first_line = true;
        }
        if self.first_line {
            line = line
                .strip_prefix('\u{feff}')
                .unwrap_or(&line)
                .to_owned()
                .into();
            self.first_line = false;
        }
        if line.is_empty() {
            if self.has_data || self.comment {
                let mut message = StreamMessage::new(
                    "server",
                    if self.has_data { "event" } else { "heartbeat" },
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
            return;
        }
        if line.starts_with(':') {
            self.comment = true;
            return;
        }
        let (name, value) = line.split_once(':').unwrap_or((&line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match name {
            "data" => {
                self.has_data = true;
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => self.event = value.to_owned(),
            "id" if !value.contains('\0') => self.id = value.to_owned(),
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                if let Ok(retry) = value.parse() {
                    self.retry = Some(retry);
                }
            }
            _ => {}
        }
    }
    fn finish(&mut self, output: &mut Vec<StreamMessage>) {
        if self.cr {
            self.cr = false;
            self.end_line(output);
        }
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
    #[test]
    fn every_chunk_boundary_preserves_sse_fields_and_raw_text() {
        let wire = "\u{feff}id: 12\r\nevent: update\rdata: héllo\ndata: world\nretry: 4500\n\n: keepalive\n\ndata: tail";
        for split in 0..=wire.len() {
            let mut parser = EventParser::default();
            let mut messages = Vec::new();
            parser
                .push(&wire.as_bytes()[..split], &mut messages)
                .unwrap();
            parser
                .push(&wire.as_bytes()[split..], &mut messages)
                .unwrap();
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
    #[test]
    fn id_reset_empty_data_and_invalid_retry_follow_event_stream_rules() {
        let mut parser = EventParser::default();
        let mut messages = Vec::new();
        parser
            .push(
                b"id: first\n\ndata:\n\nid:\nretry: -3\ndata: next\n\n",
                &mut messages,
            )
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].event_id.as_deref(), Some("first"));
        assert!(messages[0].payload.is_empty());
        assert_eq!(messages[1].event_id.as_deref(), Some(""));
        assert_eq!(messages[1].retry, None);
    }
}
