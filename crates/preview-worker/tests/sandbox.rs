#![cfg(windows)]

//! Windows `AppContainer` integration test for the raster preview worker.

use std::{
    io::{Cursor, Read, Write},
    process::{Command, Stdio},
};

use image::ImageFormat;

#[test]
fn appcontainer_worker_normalizes_one_image_without_ambient_authority() {
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_transmog-preview-worker"));
    let image = image::DynamicImage::new_rgba8(2, 2);
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, ImageFormat::WebP).unwrap();
    let encoded = encoded.into_inner();

    let mut child = Command::new(executable)
        .arg("--sandbox-bootstrap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&u32::try_from(encoded.len()).unwrap().to_be_bytes())
        .unwrap();
    input.write_all(&encoded).unwrap();
    drop(input);

    let mut output = child.stdout.take().unwrap();
    let mut header = [0_u8; 5];
    output.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 0);
    let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    assert!(length <= transmog_preview_worker::MAX_OUTPUT_BYTES);
    let mut png = vec![0_u8; length];
    output.read_exact(&mut png).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(child.wait().unwrap().success());
}
