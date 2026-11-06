//! Build-owned compressed resources for the application origin.

use std::{io::Read, sync::OnceLock};

use crate::UiError;

/// The per-resource ceiling for assets and the protocol, enforced by the build.
const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;

struct EmbeddedAsset {
    path: &'static str,
    content_type: &'static str,
    compressed: &'static [u8],
    decoded_len: usize,
    decoded: OnceLock<Result<Vec<u8>, String>>,
}

impl EmbeddedAsset {
    const fn new(
        path: &'static str,
        content_type: &'static str,
        compressed: &'static [u8],
        decoded_len: usize,
    ) -> Self {
        Self {
            path,
            content_type,
            compressed,
            decoded_len,
            decoded: OnceLock::new(),
        }
    }

    fn bytes(&self) -> Result<&[u8], UiError> {
        self.decoded
            .get_or_init(|| decompress(self.compressed, self.decoded_len))
            .as_deref()
            .map_err(|message| UiError::InvalidAsset(format!("{}: {message}", self.path)))
    }
}

fn decompress(compressed: &[u8], decoded_len: usize) -> Result<Vec<u8>, String> {
    if decoded_len > MAX_ASSET_BYTES {
        return Err("decoded asset exceeds its byte limit".to_owned());
    }
    let mut decoded = Vec::with_capacity(decoded_len + 1);
    // Bound decoded output to the build-recorded length plus one detection
    // byte, so an oversized stream cannot grow the response without limit.
    brotli::Decompressor::new(compressed, 4096)
        .take(decoded_len as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| error.to_string())?;
    if decoded.len() != decoded_len {
        return Err("decoded asset length differs from its build metadata".to_owned());
    }
    Ok(decoded)
}

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub(super) fn respond(method: &str, path: &str) -> Result<Option<crate::UiResponse>, UiError> {
    let Some(asset) = EMBEDDED_ASSETS.iter().find(|asset| asset.path == path) else {
        return Ok(None);
    };
    // HEAD needs only metadata; it does not decompress or warm the cache.
    let body = if method == "HEAD" {
        Vec::new()
    } else {
        asset.bytes()?.to_vec()
    };
    Ok(Some(crate::UiResponse::asset(
        200,
        asset.content_type,
        body,
    )))
}

pub(super) fn protocol_bytes() -> Result<Vec<u8>, UiError> {
    // Parsing owns the resulting Protocol; discard this temporary buffer rather
    // than retaining a second decompressed copy in an asset cache.
    decompress(PROTOCOL_BYTES, PROTOCOL_DECODED_LEN).map_err(UiError::InvalidProtocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppRenderer, ShellView};

    #[test]
    fn compressed_protocol_matches_the_original_build_output() {
        assert_eq!(protocol_bytes().unwrap(), ORIGINAL_PROTOCOL);
        assert!(PROTOCOL_BYTES.len() < ORIGINAL_PROTOCOL.len());
    }

    #[test]
    fn invalid_protocol_streams_and_lengths_fail_bounded_decoding() {
        for (bytes, length) in [
            (b"invalid".as_slice(), PROTOCOL_DECODED_LEN),
            (
                &PROTOCOL_BYTES[..PROTOCOL_BYTES.len() / 2],
                PROTOCOL_DECODED_LEN,
            ),
            (PROTOCOL_BYTES, 0),
            (PROTOCOL_BYTES, MAX_ASSET_BYTES + 1),
        ] {
            assert!(decompress(bytes, length).is_err());
        }
    }

    #[test]
    fn all_embedded_responses_match_original_build_outputs() {
        let renderer = AppRenderer::new().unwrap();
        let view = ShellView::from(&crate::tests::status());
        assert!(
            EMBEDDED_ASSETS
                .iter()
                .any(|asset| asset.path.starts_with("/chunks/"))
        );
        assert_eq!(EMBEDDED_ASSETS.len(), ORIGINAL_ASSETS.len());
        for (asset, (path, original)) in EMBEDDED_ASSETS.iter().zip(ORIGINAL_ASSETS) {
            assert_eq!(asset.path, *path);
            let get = renderer.respond("GET", path, &view).unwrap();
            assert_eq!(get.status, 200);
            assert_eq!(get.content_type, asset.content_type);
            assert_eq!(get.cache_control, "no-store");
            assert_eq!(get.body, *original, "response bytes changed for {path}");
            assert_eq!(
                renderer
                    .respond("GET", &format!("{path}?v=1"), &view)
                    .unwrap(),
                get
            );
            let head = renderer.respond("HEAD", path, &view).unwrap();
            assert_eq!(head.status, get.status);
            assert_eq!(head.content_type, get.content_type);
            assert_eq!(head.cache_control, get.cache_control);
            assert_eq!(head.body, [] as [u8; 0]);
            assert_eq!(renderer.respond("POST", path, &view).unwrap().status, 405);
        }
    }

    #[test]
    fn invalid_assets_fail_with_bounded_decoding() {
        let asset = &EMBEDDED_ASSETS[0];
        for invalid in [
            EmbeddedAsset::new(
                asset.path,
                asset.content_type,
                b"invalid",
                asset.decoded_len,
            ),
            EmbeddedAsset::new(asset.path, asset.content_type, asset.compressed, 0),
            EmbeddedAsset::new(
                asset.path,
                asset.content_type,
                asset.compressed,
                MAX_ASSET_BYTES + 1,
            ),
        ] {
            assert!(matches!(invalid.bytes(), Err(UiError::InvalidAsset(_))));
        }
    }
}
