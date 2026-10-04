use bytes::{Buf, Bytes, BytesMut};
use thiserror::Error;

use crate::{CompressionError, PerMessageDeflate, PerMessageDeflateCodec};

/// Direction in which a frame or message travels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Direction {
    /// Browser/client to origin server.
    ClientToServer,
    /// Origin server to browser/client.
    ServerToClient,
}

impl Direction {
    /// Whether frames sent in this direction must carry an RFC 6455 mask.
    pub fn requires_mask(self) -> bool {
        self == Self::ClientToServer
    }
}

/// Finite parser and assembled-message bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameLimits {
    /// Largest accepted wire payload in one frame.
    pub max_frame_payload_bytes: usize,
    /// Largest accepted assembled, decompressed message.
    pub max_message_payload_bytes: usize,
    /// Largest unread byte accumulation before parsing must make progress.
    pub max_buffered_bytes: usize,
}

impl Default for FrameLimits {
    fn default() -> Self {
        Self {
            max_frame_payload_bytes: 4 * 1024 * 1024,
            max_message_payload_bytes: 16 * 1024 * 1024,
            max_buffered_bytes: 4 * 1024 * 1024 + 14,
        }
    }
}

impl FrameLimits {
    fn validate(self) -> Result<Self, DecodeError> {
        if self.max_frame_payload_bytes == 0
            || self.max_message_payload_bytes == 0
            || self.max_buffered_bytes < self.max_frame_payload_bytes.saturating_add(14)
        {
            return Err(DecodeError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Application data opcode after fragment assembly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataKind {
    /// UTF-8 text.
    Text,
    /// Uninterpreted binary bytes.
    Binary,
}

/// Parsed WebSocket frame with masking removed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    /// Final-fragment flag.
    pub fin: bool,
    /// `permessage-deflate` marker on the first data frame.
    pub compressed: bool,
    /// Wire opcode.
    pub opcode: u8,
    /// Unmasked payload.
    pub payload: Bytes,
}

/// Validated close control payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseFrame {
    /// Optional RFC close status.
    pub code: Option<u16>,
    /// UTF-8 close reason without the status prefix.
    pub reason: String,
}

impl CloseFrame {
    /// Serializes the close payload.
    ///
    /// # Errors
    ///
    /// Rejects reserved status codes and payloads larger than 125 bytes.
    pub fn encode(&self) -> Result<Bytes, EncodeError> {
        let Some(code) = self.code else {
            if self.reason.is_empty() {
                return Ok(Bytes::new());
            }
            return Err(EncodeError::ReasonWithoutCode);
        };
        if !valid_close_code(code) {
            return Err(EncodeError::InvalidCloseCode(code));
        }
        let length = 2_usize
            .checked_add(self.reason.len())
            .ok_or(EncodeError::PayloadTooLarge)?;
        if length > 125 {
            return Err(EncodeError::PayloadTooLarge);
        }
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&code.to_be_bytes());
        output.extend_from_slice(self.reason.as_bytes());
        Ok(Bytes::from(output))
    }
}

/// Parsed control-frame event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlFrame {
    /// Close handshake frame.
    Close(CloseFrame),
    /// Ping payload.
    Ping(Bytes),
    /// Pong payload.
    Pong(Bytes),
}

/// Complete, decompressed application message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    /// Text or binary interpretation.
    pub kind: DataKind,
    /// Decompressed payload supplied to hooks.
    pub payload: Bytes,
    /// Whether the incoming wire representation used `permessage-deflate`.
    pub was_compressed: bool,
}

/// Next semantic item emitted from arbitrary transport chunks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageEvent {
    /// Complete application message.
    Message(Message),
    /// Control frame, which may be interleaved with fragmented data.
    Control(ControlFrame),
}

/// Incremental RFC 6455 frame decoder.
#[derive(Debug)]
pub struct FrameDecoder {
    direction: Direction,
    limits: FrameLimits,
    compression_enabled: bool,
    buffer: BytesMut,
}

impl FrameDecoder {
    /// Creates a bounded directional decoder.
    ///
    /// # Errors
    ///
    /// Rejects inconsistent or zero limits.
    pub fn new(
        direction: Direction,
        limits: FrameLimits,
        compression_enabled: bool,
    ) -> Result<Self, DecodeError> {
        Ok(Self {
            direction,
            limits: limits.validate()?,
            compression_enabled,
            buffer: BytesMut::new(),
        })
    }

    /// Appends an arbitrary transport chunk without exceeding the configured
    /// unread-buffer bound.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::BufferLimitExceeded`] before growing beyond the
    /// declared limit.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), DecodeError> {
        let attempted = self
            .buffer
            .len()
            .checked_add(bytes.len())
            .ok_or(DecodeError::BufferLimitExceeded)?;
        if attempted > self.limits.max_buffered_bytes {
            return Err(DecodeError::BufferLimitExceeded);
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    /// Parses one complete frame, retaining an incomplete suffix.
    ///
    /// # Errors
    ///
    /// Rejects non-canonical lengths, masking violations, reserved bits,
    /// invalid opcodes, oversized frames, and invalid control framing.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
        if self.buffer.len() < 2 {
            return Ok(None);
        }
        let first = self.buffer[0];
        let second = self.buffer[1];
        let fin = first & 0x80 != 0;
        let rsv1 = first & 0x40 != 0;
        if first & 0x30 != 0 {
            return Err(DecodeError::ReservedBits);
        }
        let opcode = first & 0x0f;
        if !matches!(opcode, 0x0 | 0x1 | 0x2 | 0x8 | 0x9 | 0xa) {
            return Err(DecodeError::InvalidOpcode(opcode));
        }
        if rsv1 && (!self.compression_enabled || !matches!(opcode, 0x1 | 0x2)) {
            return Err(DecodeError::UnexpectedCompression);
        }
        let control = opcode & 0x08 != 0;
        if control && (!fin || rsv1) {
            return Err(DecodeError::InvalidControlFrame);
        }
        let masked = second & 0x80 != 0;
        if masked != self.direction.requires_mask() {
            return Err(DecodeError::InvalidMasking);
        }

        let marker = second & 0x7f;
        let (payload_length, extended) = match marker {
            value @ 0..=125 => (u64::from(value), 0_usize),
            126 => {
                if self.buffer.len() < 4 {
                    return Ok(None);
                }
                let value = u16::from_be_bytes([self.buffer[2], self.buffer[3]]);
                if value < 126 {
                    return Err(DecodeError::NonCanonicalLength);
                }
                (u64::from(value), 2)
            }
            127 => {
                if self.buffer.len() < 10 {
                    return Ok(None);
                }
                let value = u64::from_be_bytes([
                    self.buffer[2],
                    self.buffer[3],
                    self.buffer[4],
                    self.buffer[5],
                    self.buffer[6],
                    self.buffer[7],
                    self.buffer[8],
                    self.buffer[9],
                ]);
                if u16::try_from(value).is_ok() || value & (1_u64 << 63) != 0 {
                    return Err(DecodeError::NonCanonicalLength);
                }
                (value, 8)
            }
            _ => unreachable!(),
        };
        let payload_length =
            usize::try_from(payload_length).map_err(|_| DecodeError::FrameLimitExceeded)?;
        if payload_length > self.limits.max_frame_payload_bytes {
            return Err(DecodeError::FrameLimitExceeded);
        }
        if control && payload_length > 125 {
            return Err(DecodeError::InvalidControlFrame);
        }
        let mask_length = usize::from(masked) * 4;
        let header_length = 2 + extended + mask_length;
        let total = header_length
            .checked_add(payload_length)
            .ok_or(DecodeError::FrameLimitExceeded)?;
        if self.buffer.len() < total {
            return Ok(None);
        }
        let mask = masked.then(|| {
            let start = 2 + extended;
            [
                self.buffer[start],
                self.buffer[start + 1],
                self.buffer[start + 2],
                self.buffer[start + 3],
            ]
        });
        let mut bytes = self.buffer.split_to(total);
        bytes.advance(header_length);
        let mut payload = bytes.to_vec();
        if let Some(mask) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        Ok(Some(Frame {
            fin,
            compressed: rsv1,
            opcode,
            payload: Bytes::from(payload),
        }))
    }
}

/// Incremental frame and fragmented-message decoder.
#[derive(Debug)]
pub struct MessageDecoder {
    frames: FrameDecoder,
    limits: FrameLimits,
    inflater: Option<PerMessageDeflateCodec>,
    fragmented: Option<FragmentedMessage>,
    close_seen: bool,
}

#[derive(Debug)]
struct FragmentedMessage {
    kind: DataKind,
    compressed: bool,
    payload: Vec<u8>,
}

impl MessageDecoder {
    /// Creates a decoder for one direction and negotiated compression policy.
    ///
    /// # Errors
    ///
    /// Rejects invalid resource limits or a compression window unsupported by
    /// the active codec.
    pub fn new(
        direction: Direction,
        limits: FrameLimits,
        compression: Option<PerMessageDeflate>,
    ) -> Result<Self, DecodeError> {
        let inflater = compression
            .map(|negotiated| PerMessageDeflateCodec::new(negotiated, direction))
            .transpose()?;
        Ok(Self {
            frames: FrameDecoder::new(direction, limits, compression.is_some())?,
            limits,
            inflater,
            fragmented: None,
            close_seen: false,
        })
    }

    /// Consumes one arbitrary transport chunk and emits every complete item.
    ///
    /// # Errors
    ///
    /// Rejects framing, fragmentation, decompression, UTF-8, close-payload,
    /// and configured resource-limit violations.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<MessageEvent>, DecodeError> {
        if self.close_seen && !bytes.is_empty() {
            return Err(DecodeError::DataAfterClose);
        }
        self.frames.push(bytes)?;
        let mut output = Vec::new();
        while let Some(frame) = self.frames.next_frame()? {
            if self.close_seen {
                return Err(DecodeError::DataAfterClose);
            }
            if frame.opcode & 0x08 != 0 {
                let control = parse_control(frame.opcode, frame.payload)?;
                if matches!(control, ControlFrame::Close(_)) {
                    self.close_seen = true;
                }
                output.push(MessageEvent::Control(control));
                continue;
            }
            match frame.opcode {
                0x0 => {
                    let fragmented = self
                        .fragmented
                        .as_mut()
                        .ok_or(DecodeError::UnexpectedContinuation)?;
                    extend_bounded(
                        &mut fragmented.payload,
                        &frame.payload,
                        self.limits.max_message_payload_bytes,
                    )?;
                    if frame.fin {
                        let Some(complete) = self.fragmented.take() else {
                            return Err(DecodeError::UnexpectedContinuation);
                        };
                        output.push(MessageEvent::Message(self.finish_message(
                            complete.kind,
                            complete.compressed,
                            complete.payload,
                        )?));
                    }
                }
                0x1 | 0x2 => {
                    if self.fragmented.is_some() {
                        return Err(DecodeError::NestedDataMessage);
                    }
                    let kind = if frame.opcode == 0x1 {
                        DataKind::Text
                    } else {
                        DataKind::Binary
                    };
                    if frame.fin {
                        output.push(MessageEvent::Message(self.finish_message(
                            kind,
                            frame.compressed,
                            frame.payload.to_vec(),
                        )?));
                    } else {
                        let mut payload = Vec::with_capacity(frame.payload.len());
                        extend_bounded(
                            &mut payload,
                            &frame.payload,
                            self.limits.max_message_payload_bytes,
                        )?;
                        self.fragmented = Some(FragmentedMessage {
                            kind,
                            compressed: frame.compressed,
                            payload,
                        });
                    }
                }
                _ => unreachable!("frame decoder filtered opcode"),
            }
        }
        Ok(output)
    }

    fn finish_message(
        &mut self,
        kind: DataKind,
        compressed: bool,
        payload: Vec<u8>,
    ) -> Result<Message, DecodeError> {
        let payload = if compressed {
            self.inflater
                .as_mut()
                .ok_or(DecodeError::UnexpectedCompression)?
                .decompress(&payload, self.limits.max_message_payload_bytes)?
        } else {
            payload
        };
        if payload.len() > self.limits.max_message_payload_bytes {
            return Err(DecodeError::MessageLimitExceeded);
        }
        if kind == DataKind::Text && std::str::from_utf8(&payload).is_err() {
            return Err(DecodeError::InvalidUtf8);
        }
        Ok(Message {
            kind,
            payload: Bytes::from(payload),
            was_compressed: compressed,
        })
    }
}

fn extend_bounded(target: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), DecodeError> {
    let attempted = target
        .len()
        .checked_add(bytes.len())
        .ok_or(DecodeError::MessageLimitExceeded)?;
    if attempted > limit {
        return Err(DecodeError::MessageLimitExceeded);
    }
    target.extend_from_slice(bytes);
    Ok(())
}

fn parse_control(opcode: u8, payload: Bytes) -> Result<ControlFrame, DecodeError> {
    Ok(match opcode {
        0x8 => {
            if payload.len() == 1 {
                return Err(DecodeError::InvalidClosePayload);
            }
            let (code, reason) = if payload.is_empty() {
                (None, String::new())
            } else {
                let code = u16::from_be_bytes([payload[0], payload[1]]);
                if !valid_close_code(code) {
                    return Err(DecodeError::InvalidCloseCode(code));
                }
                let reason = std::str::from_utf8(&payload[2..])
                    .map_err(|_| DecodeError::InvalidUtf8)?
                    .to_owned();
                (Some(code), reason)
            };
            ControlFrame::Close(CloseFrame { code, reason })
        }
        0x9 => ControlFrame::Ping(payload),
        0xa => ControlFrame::Pong(payload),
        _ => unreachable!("frame decoder filtered control opcode"),
    })
}

fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

/// Encodes one validated frame with directionally correct masking.
///
/// # Errors
///
/// Rejects invalid opcodes, control framing, oversized control payloads, and a
/// missing or unexpected mask key.
pub fn encode_frame(
    frame: &Frame,
    direction: Direction,
    mask: Option<[u8; 4]>,
) -> Result<Vec<u8>, EncodeError> {
    if !matches!(frame.opcode, 0x0 | 0x1 | 0x2 | 0x8 | 0x9 | 0xa) {
        return Err(EncodeError::InvalidOpcode(frame.opcode));
    }
    let control = frame.opcode & 0x08 != 0;
    if control && (!frame.fin || frame.compressed || frame.payload.len() > 125) {
        return Err(EncodeError::InvalidControlFrame);
    }
    if frame.compressed && !matches!(frame.opcode, 0x1 | 0x2) {
        return Err(EncodeError::UnexpectedCompression);
    }
    if mask.is_some() != direction.requires_mask() {
        return Err(EncodeError::InvalidMasking);
    }
    let mut output = Vec::with_capacity(frame.payload.len().saturating_add(14));
    let mut first = frame.opcode;
    if frame.fin {
        first |= 0x80;
    }
    if frame.compressed {
        first |= 0x40;
    }
    output.push(first);
    let mask_bit = if mask.is_some() { 0x80 } else { 0 };
    match frame.payload.len() {
        length @ 0..=125 => {
            output.push(mask_bit | u8::try_from(length).map_err(|_| EncodeError::PayloadTooLarge)?);
        }
        length @ 126..=65_535 => {
            output.push(mask_bit | 0x7e);
            output.extend_from_slice(
                &u16::try_from(length)
                    .map_err(|_| EncodeError::PayloadTooLarge)?
                    .to_be_bytes(),
            );
        }
        length => {
            output.push(mask_bit | 0x7f);
            output.extend_from_slice(
                &u64::try_from(length)
                    .map_err(|_| EncodeError::PayloadTooLarge)?
                    .to_be_bytes(),
            );
        }
    }
    if let Some(mask) = mask {
        output.extend_from_slice(&mask);
        output.extend(
            frame
                .payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
    } else {
        output.extend_from_slice(&frame.payload);
    }
    Ok(output)
}

/// Encodes an application message into one or more bounded frames.
///
/// `wire_payload` is already compressed when `compressed` is true. A fresh,
/// unpredictable mask is requested for every client-to-server fragment.
///
/// # Errors
///
/// Rejects a zero fragment bound or propagates mask-generation/encoding
/// failures.
pub fn encode_message(
    kind: DataKind,
    wire_payload: &[u8],
    compressed: bool,
    direction: Direction,
    max_frame_payload_bytes: usize,
    next_mask: &mut dyn FnMut() -> Result<[u8; 4], EncodeError>,
) -> Result<Vec<Vec<u8>>, EncodeError> {
    if max_frame_payload_bytes == 0 {
        return Err(EncodeError::InvalidLimits);
    }
    let chunks = wire_payload.len().max(1).div_ceil(max_frame_payload_bytes);
    let mut output = Vec::with_capacity(chunks);
    for index in 0..chunks {
        let start = index * max_frame_payload_bytes;
        let end = wire_payload
            .len()
            .min(start.saturating_add(max_frame_payload_bytes));
        let opcode = if index == 0 {
            match kind {
                DataKind::Text => 0x1,
                DataKind::Binary => 0x2,
            }
        } else {
            0x0
        };
        let mask = if direction.requires_mask() {
            Some(next_mask()?)
        } else {
            None
        };
        output.push(encode_frame(
            &Frame {
                fin: index + 1 == chunks,
                compressed: compressed && index == 0,
                opcode,
                payload: Bytes::copy_from_slice(&wire_payload[start..end]),
            },
            direction,
            mask,
        )?);
    }
    Ok(output)
}

/// Frame or message decoding failure.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// Resource limits were zero or inconsistent.
    #[error("invalid WebSocket frame limits")]
    InvalidLimits,
    /// Unread input would exceed its bound.
    #[error("WebSocket unread buffer limit exceeded")]
    BufferLimitExceeded,
    /// Frame payload exceeded its bound.
    #[error("WebSocket frame payload limit exceeded")]
    FrameLimitExceeded,
    /// Assembled or decompressed message exceeded its bound.
    #[error("WebSocket message payload limit exceeded")]
    MessageLimitExceeded,
    /// RSV2 or RSV3 was set.
    #[error("unexpected WebSocket reserved bits")]
    ReservedBits,
    /// RSV1 was used without a valid compression context.
    #[error("unexpected WebSocket compression marker")]
    UnexpectedCompression,
    /// Opcode is reserved or unknown.
    #[error("invalid WebSocket opcode {0:#x}")]
    InvalidOpcode(u8),
    /// Control frame was fragmented, compressed, or too large.
    #[error("invalid WebSocket control frame")]
    InvalidControlFrame,
    /// Mask bit did not match the sender role.
    #[error("invalid WebSocket masking for direction")]
    InvalidMasking,
    /// Extended length used a non-minimal encoding or set the high bit.
    #[error("non-canonical WebSocket payload length")]
    NonCanonicalLength,
    /// Continuation arrived without a fragmented message.
    #[error("unexpected WebSocket continuation frame")]
    UnexpectedContinuation,
    /// New data opcode arrived while a fragmented message was open.
    #[error("nested fragmented WebSocket message")]
    NestedDataMessage,
    /// Text or close reason was not valid UTF-8.
    #[error("invalid WebSocket UTF-8")]
    InvalidUtf8,
    /// Close payload had exactly one byte.
    #[error("invalid WebSocket close payload")]
    InvalidClosePayload,
    /// Close status cannot appear on the wire.
    #[error("invalid WebSocket close code {0}")]
    InvalidCloseCode(u16),
    /// Bytes arrived in one direction after its close frame.
    #[error("WebSocket data arrived after close")]
    DataAfterClose,
    /// Message compression failed or exceeded its bound.
    #[error(transparent)]
    Compression(#[from] CompressionError),
}

/// Frame encoding failure.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// Fragment size was zero.
    #[error("invalid WebSocket encoding limits")]
    InvalidLimits,
    /// Opcode is reserved or unknown.
    #[error("invalid WebSocket opcode {0:#x}")]
    InvalidOpcode(u8),
    /// Control frame was fragmented, compressed, or too large.
    #[error("invalid WebSocket control frame")]
    InvalidControlFrame,
    /// Compression marker was set on a non-initial data frame.
    #[error("unexpected WebSocket compression marker")]
    UnexpectedCompression,
    /// Mask presence did not match the sender direction.
    #[error("invalid WebSocket masking for direction")]
    InvalidMasking,
    /// Payload could not be represented safely.
    #[error("WebSocket payload is too large")]
    PayloadTooLarge,
    /// A close reason was supplied without a status code.
    #[error("WebSocket close reason requires a status code")]
    ReasonWithoutCode,
    /// Close status cannot appear on the wire.
    #[error("invalid WebSocket close code {0}")]
    InvalidCloseCode(u16),
    /// Cryptographic mask generation failed.
    #[error("WebSocket mask generation failed")]
    MaskGeneration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_frame(frame: &Frame) -> Vec<u8> {
        encode_frame(
            frame,
            Direction::ClientToServer,
            Some([0x37, 0xfa, 0x21, 0x3d]),
        )
        .unwrap()
    }

    #[test]
    fn rfc_mask_vector_decodes_at_every_tcp_boundary() {
        let wire = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        for split in 0..=wire.len() {
            let mut decoder =
                MessageDecoder::new(Direction::ClientToServer, FrameLimits::default(), None)
                    .unwrap();
            let mut events = decoder.push(&wire[..split]).unwrap();
            events.extend(decoder.push(&wire[split..]).unwrap());
            assert_eq!(
                events,
                [MessageEvent::Message(Message {
                    kind: DataKind::Text,
                    payload: Bytes::from_static(b"Hello"),
                    was_compressed: false,
                })]
            );
        }
    }

    #[test]
    fn fragmented_text_allows_interleaved_ping() {
        let first = client_frame(&Frame {
            fin: false,
            compressed: false,
            opcode: 1,
            payload: Bytes::from_static(b"Hel"),
        });
        let ping = client_frame(&Frame {
            fin: true,
            compressed: false,
            opcode: 9,
            payload: Bytes::from_static(b"?"),
        });
        let last = client_frame(&Frame {
            fin: true,
            compressed: false,
            opcode: 0,
            payload: Bytes::from_static(b"lo"),
        });
        let mut decoder =
            MessageDecoder::new(Direction::ClientToServer, FrameLimits::default(), None).unwrap();
        assert!(decoder.push(&first).unwrap().is_empty());
        assert_eq!(
            decoder.push(&ping).unwrap(),
            [MessageEvent::Control(ControlFrame::Ping(
                Bytes::from_static(b"?")
            ))]
        );
        assert_eq!(
            decoder.push(&last).unwrap(),
            [MessageEvent::Message(Message {
                kind: DataKind::Text,
                payload: Bytes::from_static(b"Hello"),
                was_compressed: false,
            })]
        );
    }

    #[test]
    fn masking_direction_and_noncanonical_lengths_fail() {
        let mut server =
            FrameDecoder::new(Direction::ServerToClient, FrameLimits::default(), false).unwrap();
        server.push(&[0x81, 0x80, 1, 2, 3, 4]).unwrap();
        assert!(matches!(
            server.next_frame(),
            Err(DecodeError::InvalidMasking)
        ));

        let mut client =
            FrameDecoder::new(Direction::ClientToServer, FrameLimits::default(), false).unwrap();
        client.push(&[0x82, 0xfe, 0, 125, 1, 2, 3, 4]).unwrap();
        assert!(matches!(
            client.next_frame(),
            Err(DecodeError::NonCanonicalLength)
        ));
    }

    #[test]
    fn close_and_utf8_are_validated() {
        let mut decoder =
            MessageDecoder::new(Direction::ServerToClient, FrameLimits::default(), None).unwrap();
        assert!(matches!(
            decoder.push(&[0x88, 0x01, 0]),
            Err(DecodeError::InvalidClosePayload)
        ));

        let mut decoder =
            MessageDecoder::new(Direction::ServerToClient, FrameLimits::default(), None).unwrap();
        assert!(matches!(
            decoder.push(&[0x81, 0x02, 0xc3, 0x28]),
            Err(DecodeError::InvalidUtf8)
        ));
    }

    #[test]
    fn encoder_fragments_and_masks_each_client_frame() {
        let mut count = 0_u8;
        let encoded = encode_message(
            DataKind::Binary,
            b"abcdef",
            false,
            Direction::ClientToServer,
            2,
            &mut || {
                count += 1;
                Ok([count; 4])
            },
        )
        .unwrap();
        assert_eq!(encoded.len(), 3);
        assert_eq!(encoded[0][0], 0x02);
        assert_eq!(encoded[1][0], 0x00);
        assert_eq!(encoded[2][0], 0x80);
        assert!(encoded.iter().all(|frame| frame[1] & 0x80 != 0));
    }
}
