//! Compare incremental SSE observation with whole-buffer decoding and compression.
use bytes::Bytes;
use std::sync::OnceLock;
use transmog_content::{ContentCoding, ContentEncoder, ContentLimits, sse::SseDecoder};
use transmog_core::{BodyFrame, HeaderBlock, HeaderField, stream::StreamMessage};

async fn decode(wire: &[u8], headers: &HeaderBlock, chunk: usize) -> Vec<StreamMessage> {
    let mut decoder = SseDecoder::new(headers).unwrap();
    let mut messages = Vec::new();
    for part in wire.chunks(chunk) {
        messages.extend(
            decoder
                .push(BodyFrame::Data(Bytes::copy_from_slice(part)))
                .await
                .unwrap(),
        );
    }
    messages.extend(decoder.finish().await.unwrap());
    for message in &mut messages {
        message.at_millis = 0;
        assert!(!message.truncated);
        assert_eq!(message.byte_count, message.payload.len() as u64);
    }
    messages
}

pub(crate) fn exercise(data: &[u8]) {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let Some((&flags, data)) = data.split_first() else {
        return;
    };
    let Some((&chunk, data)) = data.split_first() else {
        return;
    };
    if data.len() > 8192 {
        return;
    }
    // Exercise SSE chunking, not expensive Brotli fixture generation. Raw SSE
    // still uses the full input; larger codec/session cases have separate tests.
    let data = if flags & 2 != 0 {
        &data[..data.len().min(512)]
    } else {
        data
    };
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        })
        .block_on(async {
            let mut wire = data.to_vec();
            if flags & 1 != 0 {
                wire = b"\xef\xbb\xbfid: ".to_vec();
                wire.extend_from_slice(data);
                wire.extend_from_slice(
                    b"\r\ndata: \xf0\x9f\x8c\x8d\r\r: heartbeat\n\nid:\nretry: 42\ndata:\n\n",
                );
            }
            let expected = decode(&wire, &HeaderBlock::default(), wire.len().max(1)).await;
            let headers = if flags & 2 != 0 {
                let coding = [
                    ContentCoding::Gzip,
                    ContentCoding::Deflate,
                    ContentCoding::Brotli,
                    ContentCoding::Zstd,
                ][usize::from(flags >> 2) % 4];
                let name = match coding {
                    ContentCoding::Gzip => "gzip",
                    ContentCoding::Deflate => "deflate",
                    ContentCoding::Brotli => "br",
                    ContentCoding::Zstd => "zstd",
                };
                let mut encoder = ContentEncoder::new(coding, ContentLimits::default()).unwrap();
                let mut frames = encoder
                    .on_frame(BodyFrame::Data(Bytes::from(wire)))
                    .await
                    .unwrap();
                frames.extend(encoder.finish().await.unwrap());
                wire = frames
                    .into_iter()
                    .flat_map(|frame| match frame {
                        BodyFrame::Data(bytes) => bytes.to_vec(),
                        BodyFrame::Trailers(_) => Vec::new(),
                    })
                    .collect();
                HeaderBlock::from_fields(vec![
                    HeaderField::try_new("content-encoding", name).unwrap(),
                ])
            } else {
                HeaderBlock::default()
            };
            let actual = decode(&wire, &headers, usize::from(chunk) + 1).await;
            assert_eq!(actual, expected);
        });
}
