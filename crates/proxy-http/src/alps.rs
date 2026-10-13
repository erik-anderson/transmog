//! Adapt TLS-delivered H2 settings to h2's normal settings processing.
//!
//! h2 has no ALPS bootstrap API. A single local SETTINGS frame seeds its decoder;
//! the corresponding ACK is consumed locally because TLS already acknowledged
//! these settings. Real network SETTINGS and their ACKs remain untouched. The
//! client starts with zero send-stream slots so ALPS is applied before encoding
//! any request. No fork of h2, Hyper, or hyper-util is needed.
use bytes::{Buf, Bytes};
use hyper_util::{
    client::legacy::connect::{Connected, Connection},
    rt::TokioIo,
};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const ACK: [u8; 9] = [0, 0, 0, 4, 1, 0, 0, 0, 0];

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Validate framing and values before allowing the HTTP layer to send requests.
/// `ACCEPT_CH` is an optional browser optimization. We discard its preferences:
/// this proxy has neither the caller's hint values nor its permissions policy.
pub(crate) fn bootstrap(data: &[u8]) -> io::Result<Option<Bytes>> {
    if data.len() > 65_535 {
        return Err(invalid("HTTP/2 ALPS exceeds the TLS settings bound"));
    }
    let mut data = data;
    let mut settings = [None; 9];
    let mut received_settings = false;
    while !data.is_empty() {
        if data.len() < 9 {
            return Err(invalid("truncated HTTP/2 ALPS frame header"));
        }
        let length = usize::from(data[0]) << 16 | usize::from(data[1]) << 8 | usize::from(data[2]);
        let kind = data[3];
        let flags = data[4];
        let stream = u32::from_be_bytes(data[5..9].try_into().expect("frame header")) & 0x7fff_ffff;
        data = &data[9..];
        if length > 16_384 || data.len() < length {
            return Err(invalid("invalid HTTP/2 ALPS frame length"));
        }
        let payload = &data[..length];
        data = &data[length..];
        match kind {
            4 => {
                if stream != 0 || flags & 1 != 0 || length % 6 != 0 {
                    return Err(invalid("invalid HTTP/2 ALPS SETTINGS frame"));
                }
                received_settings = true;
                for parameter in payload.chunks_exact(6) {
                    let id = usize::from(u16::from_be_bytes(
                        parameter[..2].try_into().expect("setting id"),
                    ));
                    let value =
                        u32::from_be_bytes(parameter[2..].try_into().expect("setting value"));
                    if matches!(id, 2 | 8) && value > 1
                        || id == 4 && value > 0x7fff_ffff
                        || id == 5 && !(16_384..=16_777_215).contains(&value)
                    {
                        return Err(invalid("invalid HTTP/2 ALPS setting value"));
                    }
                    // Validate every occurrence before merging; a later valid
                    // duplicate must not hide an invalid earlier value.
                    if matches!(id, 1..=6 | 8) {
                        settings[id] = Some(value);
                    }
                }
            }
            0x89 => {
                if stream != 0 || flags != 0 {
                    return Err(invalid("invalid HTTP/2 ALPS ACCEPT_CH frame"));
                }
            }
            0..=9 => return Err(invalid("forbidden frame in HTTP/2 ALPS")),
            _ => {} // Unknown extension frames are ignored, as in Chromium.
        }
    }
    if !received_settings {
        return Ok(None);
    }
    let mut frame = vec![0, 0, 0, 4, 0, 0, 0, 0, 0];
    for (id, value) in settings.into_iter().enumerate() {
        if let Some(value) = value {
            frame.extend_from_slice(&u16::try_from(id).expect("known setting").to_be_bytes());
            frame.extend_from_slice(&value.to_be_bytes());
        }
    }
    frame[2] = u8::try_from(frame.len() - 9).expect("seven bounded settings");
    Ok(Some(frame.into()))
}

#[derive(Default)]
struct AckFilter {
    preface: usize,
    header: [u8; 9],
    buffered: usize,
    written: usize,
    payload: usize,
}

struct AlpsIo<S> {
    inner: S,
    prefix: Bytes,
    filter: Option<AckFilter>,
}
impl<S> AlpsIo<S> {
    fn new(inner: S, prefix: Option<Bytes>) -> Self {
        let filter = prefix.as_ref().map(|_| AckFilter::default());
        Self {
            inner,
            prefix: prefix.unwrap_or_default(),
            filter,
        }
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for AlpsIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.prefix.is_empty() {
            let length = output.remaining().min(this.prefix.len());
            output.put_slice(&this.prefix[..length]);
            this.prefix.advance(length);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, output)
    }
}
impl<S: AsyncWrite + Unpin> AlpsIo<S> {
    fn drain_header(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(filter) = &mut self.filter {
            while filter.buffered == 9 && filter.written < 9 {
                let length = ready!(
                    Pin::new(&mut self.inner).poll_write(cx, &filter.header[filter.written..])
                )?;
                if length == 0 {
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
                filter.written += length;
            }
            if filter.buffered == 9 {
                filter.buffered = 0;
                filter.written = 0;
            }
        }
        Poll::Ready(Ok(()))
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for AlpsIo<S> {
    fn is_write_vectored(&self) -> bool {
        self.filter.is_none() && self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if self.filter.is_none() {
            Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, input)
        } else {
            self.poll_write(
                cx,
                input
                    .iter()
                    .find(|slice| !slice.is_empty())
                    .map_or(&[], |slice| slice.as_ref()),
            )
        }
    }
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if input.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(this.drain_header(cx))?;
        let Some(filter) = &mut this.filter else {
            return Pin::new(&mut this.inner).poll_write(cx, input);
        };
        if filter.preface < PREFACE.len() {
            let length = input.len().min(PREFACE.len() - filter.preface);
            if input[..length] != PREFACE[filter.preface..filter.preface + length] {
                return Poll::Ready(Err(invalid("invalid client HTTP/2 preface")));
            }
            let written = ready!(Pin::new(&mut this.inner).poll_write(cx, &input[..length]))?;
            filter.preface += written;
            return Poll::Ready(Ok(written));
        }
        if filter.payload > 0 {
            let length = input.len().min(filter.payload);
            let written = ready!(Pin::new(&mut this.inner).poll_write(cx, &input[..length]))?;
            filter.payload -= written;
            return Poll::Ready(Ok(written));
        }
        let length = input.len().min(9 - filter.buffered);
        filter.header[filter.buffered..filter.buffered + length].copy_from_slice(&input[..length]);
        filter.buffered += length;
        if filter.buffered == 9 {
            if filter.header == ACK {
                this.filter = None; // Exactly one locally generated ACK; all later bytes pass through.
            } else {
                filter.payload = usize::from(filter.header[0]) << 16
                    | usize::from(filter.header[1]) << 8
                    | usize::from(filter.header[2]);
            }
        }
        Poll::Ready(Ok(length))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // This adapter is private to h2's framed writer, which flushes complete
        // headers. Fail closed if that contract changes in a dependency update.
        if this
            .filter
            .as_ref()
            .is_some_and(|filter| (1..9).contains(&filter.buffered))
        {
            return Poll::Ready(Err(invalid(
                "incomplete client HTTP/2 frame header at flush",
            )));
        }
        ready!(this.drain_header(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub(crate) struct AlpsStream<S> {
    io: TokioIo<AlpsIo<TokioIo<S>>>,
}
impl<S> AlpsStream<S> {
    pub(crate) fn new(inner: S, prefix: Option<Bytes>) -> Self {
        Self {
            io: TokioIo::new(AlpsIo::new(TokioIo::new(inner), prefix)),
        }
    }
}
impl<S: Connection> Connection for AlpsStream<S> {
    fn connected(&self) -> Connected {
        self.io.inner().inner.inner().connected()
    }
}
impl<S: hyper::rt::Read + hyper::rt::Write + Unpin> hyper::rt::Read for AlpsStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(cx, output)
    }
}
impl<S: hyper::rt::Read + hyper::rt::Write + Unpin> hyper::rt::Write for AlpsStream<S> {
    fn is_write_vectored(&self) -> bool {
        hyper::rt::Write::is_write_vectored(&self.io)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        hyper::rt::Write::poll_write_vectored(Pin::new(&mut self.get_mut().io), cx, input)
    }
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, input)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn alps_rejects_truncated_frames_and_invalid_values_before_merging_duplicates() {
        let valid = [0, 0, 6, 4, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 1];
        for end in 1..valid.len() {
            assert!(
                bootstrap(&valid[..end]).is_err(),
                "accepted truncated length {end}"
            );
        }
        assert!(bootstrap(&valid).unwrap().is_some());
        let mut duplicate = valid.to_vec();
        duplicate[11] = 0x80; // Invalid initial window, even though a later value is valid.
        duplicate.extend(valid);
        assert!(bootstrap(&duplicate).is_err());
        let mut accept_ch = vec![0, 0, 0, 0x89, 0, 0, 0, 0, 0];
        assert!(bootstrap(&accept_ch).unwrap().is_none());
        accept_ch[4] = 1;
        assert!(bootstrap(&accept_ch).is_err());
        assert!(bootstrap(&vec![0; 65_536]).is_err());
    }

    #[tokio::test]
    async fn fragmented_writes_and_backpressure_remove_only_the_bootstrap_ack() {
        let (inner, mut remote) = tokio::io::duplex(1);
        let prefix = Bytes::from_static(&[0, 0, 0, 4, 0, 0, 0, 0, 0]);
        let mut io = AlpsIo::new(inner, Some(prefix.clone()));
        let mut received_prefix = [0; 9];
        io.read_exact(&mut received_prefix).await.unwrap();
        assert_eq!(received_prefix.as_slice(), prefix.as_ref());
        let reader = tokio::spawn(async move {
            let mut actual = Vec::new();
            remote.read_to_end(&mut actual).await.unwrap();
            actual
        });
        let mut expected = PREFACE.to_vec();
        expected.extend_from_slice(&prefix);
        // The DATA payload deliberately contains an ACK-shaped sequence.
        expected.extend_from_slice(&[0, 0, 9, 0, 0, 0, 0, 0, 1]);
        expected.extend_from_slice(&ACK);
        let mut outgoing = expected.clone();
        outgoing.extend_from_slice(&ACK); // Local ACK for TLS-delivered settings.
        outgoing.extend_from_slice(&ACK); // Real ACK, which must be preserved.
        expected.extend_from_slice(&ACK);
        for chunk in outgoing.chunks(2) {
            let mut remaining = chunk;
            while !remaining.is_empty() {
                let length = io
                    .write_vectored(&[io::IoSlice::new(&[]), io::IoSlice::new(remaining)])
                    .await
                    .unwrap();
                assert!(length > 0);
                remaining = &remaining[length..];
            }
        }
        io.shutdown().await.unwrap();
        assert_eq!(reader.await.unwrap(), expected);
    }
}
