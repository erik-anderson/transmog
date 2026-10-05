#![deny(missing_docs)]

//! Bounded one-shot rasterization of hostile image bytes.

use std::io::{Cursor, Read, Write};

use image::{ImageFormat, ImageReader, Limits};

/// Maximum accepted encoded image bytes.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum emitted normalized PNG bytes.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
/// Maximum decoded width or height.
pub const MAX_DIMENSION: u32 = 8_192;
/// Maximum decoded pixels.
pub const MAX_PIXELS: u64 = 40_000_000;
/// Exact Job Object process-memory ceiling requested by the bootstrap.
pub const PROCESS_MEMORY_BYTES: usize = 384 * 1024 * 1024;

/// Decodes one image and emits a metadata-free, single-frame PNG.
///
/// # Errors
/// Returns a redaction-safe format, dimension, allocation, decode, or output
/// limit failure. SVG and every non-raster/unsupported format are rejected.
pub fn rasterize(source: &[u8]) -> Result<Vec<u8>, String> {
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return Err("preview source exceeds its byte limit".to_owned());
    }
    let reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|_| "preview image signature is invalid".to_owned())?;
    let format = reader
        .format()
        .filter(|format| {
            matches!(
                format,
                ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
            )
        })
        .ok_or_else(|| "preview image format is not allowed".to_owned())?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| "preview image dimensions are invalid".to_owned())?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("preview image dimensions exceed their limit".to_owned());
    }

    let mut reader = ImageReader::with_format(Cursor::new(source), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "preview image decoding failed".to_owned())?;
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| "preview PNG normalization failed".to_owned())?;
    let output = output.into_inner();
    if output.len() > MAX_OUTPUT_BYTES {
        return Err("normalized preview exceeds its byte limit".to_owned());
    }
    Ok(output)
}

/// Runs the one-shot bounded binary pipe protocol.
///
/// Request: big-endian u32 length followed by exact source bytes. Response:
/// status byte (`0` success, `1` failure), big-endian u32 length, then PNG or
/// bounded UTF-8 diagnostic bytes.
///
/// # Errors
/// Returns only transport-level failures.
pub fn run(mut input: impl Read, mut output: impl Write) -> Result<(), String> {
    let length = read_length(&mut input, MAX_SOURCE_BYTES)?;
    let mut source = vec![0_u8; length];
    input
        .read_exact(&mut source)
        .map_err(|_| "preview request was truncated".to_owned())?;
    match rasterize(&source) {
        Ok(png) => write_response(&mut output, 0, &png),
        Err(error) => write_response(&mut output, 1, error.as_bytes()),
    }
}

fn read_length(input: &mut impl Read, max: usize) -> Result<usize, String> {
    let mut bytes = [0_u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|_| "preview request header was truncated".to_owned())?;
    let length = usize::try_from(u32::from_be_bytes(bytes))
        .map_err(|_| "preview request length is invalid".to_owned())?;
    if length == 0 || length > max {
        return Err("preview request length exceeds its limit".to_owned());
    }
    Ok(length)
}

fn write_response(output: &mut impl Write, status: u8, bytes: &[u8]) -> Result<(), String> {
    let length = u32::try_from(bytes.len()).map_err(|_| "preview response is too large")?;
    output
        .write_all(&[status])
        .and_then(|()| output.write_all(&length.to_be_bytes()))
        .and_then(|()| output.write_all(bytes))
        .and_then(|()| output.flush())
        .map_err(|_| "preview response transport failed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_pixel(format: ImageFormat) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(1, 1);
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn supported_rasters_become_static_pngs() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ] {
            let source = one_pixel(format);
            let result = rasterize(&source).unwrap();
            assert!(result.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }

    #[test]
    fn active_polyglots_malformed_and_limits_fail_closed() {
        for source in [
            b"<svg onload='alert(1)'></svg>".as_slice(),
            b"<html><script>alert(1)</script></html>".as_slice(),
            b"GIF89a<script>".as_slice(),
            b"\x89PNG\r\n\x1a\ntruncated".as_slice(),
        ] {
            assert!(rasterize(source).is_err());
        }
        assert!(rasterize(&vec![0; MAX_SOURCE_BYTES + 1]).is_err());
        let oversized = one_pixel_with_dimensions(MAX_DIMENSION + 1, 1);
        assert_eq!(
            rasterize(&oversized).unwrap_err(),
            "preview image dimensions exceed their limit"
        );
    }

    #[test]
    fn pipe_protocol_is_bounded_and_explicit() {
        let source = one_pixel(ImageFormat::Png);
        let mut request = Vec::new();
        request.extend_from_slice(&u32::try_from(source.len()).unwrap().to_be_bytes());
        request.extend_from_slice(&source);
        let mut response = Vec::new();
        run(request.as_slice(), &mut response).unwrap();
        assert_eq!(response[0], 0);
        assert!(response[5..].starts_with(b"\x89PNG"));
        assert!(run([0, 0, 0, 0].as_slice(), Vec::new()).is_err());
    }

    fn one_pixel_with_dimensions(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(width, height);
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
}
