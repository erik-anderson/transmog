//! Synthetic capture pipeline benchmark; JSONL output, no network traffic.
//! Run with --mib 512 --workers 1|2|4 --kind text|binary|mixed|small.
//! --wait allows the driver to set CPU affinity before any workers start.
use std::{
    collections::HashMap,
    io::{self, BufWriter, Write},
    sync::mpsc,
    time::Instant,
};
use transmog_capture::compression::{BODY_CHUNK_BYTES, BodyCompression, ChunkCompressor};

fn option(name: &str, fallback: &str) -> String {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map_or_else(|| fallback.into(), |pair| pair[1].clone())
}
// Measurements intentionally round byte/time counters to floating-point rates.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--wait") {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
    }
    let workers: usize = option("--workers", "1").parse()?;
    let mib: usize = option("--mib", "256").parse()?;
    let kind = option("--kind", "mixed");
    if !(1..=4).contains(&workers)
        || mib == 0
        || !["text", "binary", "mixed", "small"].contains(&kind.as_str())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected workers 1..4, positive MiB, and text/binary/mixed/small workload",
        )
        .into());
    }
    let level: i32 = option("--level", "1").parse()?;
    let disk = option("--sink", "file") == "file";
    let chunk = if kind == "small" {
        2048
    } else {
        BODY_CHUNK_BYTES
    };
    let jobs = mib * 1024 * 1024 / chunk;
    let mut seed = 123_456_789_u64;
    let binary: Vec<u8> = (0..chunk)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()[0]
        })
        .collect();
    let mut text = Vec::with_capacity(chunk + 256);
    let mut text_seed = 123_456_789_u64;
    while text.len() < chunk {
        text_seed ^= text_seed << 13;
        text_seed ^= text_seed >> 7;
        text_seed ^= text_seed << 17;
        let line = format!(
            "{{\"url\":\"https://example.test/assets/{}\",\"user\":{},\"value\":\"{text_seed:016x}\",\"active\":true}}\n",
            text_seed % 1024,
            text_seed % 10000
        );
        text.extend_from_slice(line.as_bytes());
    }
    text.truncate(chunk);
    if std::env::args().any(|arg| arg == "--container") {
        use transmog_capture::{
            CaptureEncoding, CaptureLimits, CapturePassword, CaptureRecord, CaptureRecordKind,
            CaptureWriter,
        };
        let encrypted = std::env::args().any(|arg| arg == "--encrypted");
        let options = CaptureEncoding {
            password: encrypted.then(|| CapturePassword::new("synthetic-benchmark".into())),
        };
        let output: Box<dyn Write> = if disk {
            Box::new(tempfile::tempfile()?)
        } else {
            Box::new(io::sink())
        };
        let mut writer = CaptureWriter::with_encoding(
            output,
            CaptureLimits {
                max_file_bytes: u64::MAX,
                ..CaptureLimits::default()
            },
            &options,
        )?;
        writer.set_compression_workers(workers)?;
        let start = Instant::now();
        let mut waits = Vec::with_capacity(jobs);
        for i in 0..jobs {
            let source = if kind == "binary" || (kind == "mixed" && i % 4 != 0) {
                &binary
            } else {
                &text
            };
            let body = if kind == "small" { i } else { i % 64 };
            let record = CaptureRecord {
                sequence: i as u64 + 1,
                exchange_id: body as u128,
                kind: CaptureRecordKind::BodySegment {
                    boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
                    byte_count: source.len(),
                    bytes: Some(source.clone()),
                    truncated: false,
                },
            };
            let before = Instant::now();
            writer.append(&record)?;
            // Periodic metadata publication is part of the measured pipeline.
            if i % 8192 == 8191 {
                writer.checkpoint()?;
            }
            waits.push(u64::try_from(before.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
        writer.seal()?;
        let elapsed = start.elapsed().as_secs_f64();
        let stored = writer.bytes_written();
        waits.sort_unstable();
        println!(
            "{}",
            serde_json::json!({"mode":"container", "encrypted":encrypted, "kind":kind,"workers":workers,"input_mib":mib,"seconds":elapsed,"mib_s":mib as f64 / elapsed,"admission_p95_us":waits[waits.len()*95/100] as f64 / 1000.0,"admission_p99_us":waits[waits.len()*99/100] as f64 / 1000.0,"stored_bytes":stored})
        );
        return Ok(());
    }
    let file = tempfile::tempfile()?;
    let output: Box<dyn Write + Send> = if disk {
        Box::new(BufWriter::with_capacity(1024 * 1024, file))
    } else {
        Box::new(io::sink())
    };
    let start = Instant::now();
    let mut waits = Vec::with_capacity(jobs);
    let mut senders = Vec::new();
    let mut results = Vec::new();
    let mut threads = Vec::new();
    for _ in 0..workers {
        let (tx, rx) = mpsc::sync_channel::<(u64, Vec<u8>)>((32 / workers).max(1));
        let (out, result) = mpsc::sync_channel((32 / workers).max(1));
        senders.push(tx);
        results.push(result);
        threads.push(std::thread::spawn(move || -> io::Result<u64> {
            let mut compressor = ChunkCompressor::new(level)?;
            let mut states = HashMap::<u64, BodyCompression>::new();
            for (body, bytes) in rx {
                let result = compressor.body(&bytes, states.entry(body).or_default())?;
                out.send(result).map_err(|_| io::ErrorKind::BrokenPipe)?;
                if states.len() > 256 {
                    states.clear();
                }
            }
            Ok(compressor.attempted_bytes())
        }));
    }
    let consumer = std::thread::spawn(move || -> io::Result<u64> {
        let mut output = output;
        let mut stored = 0;
        for i in 0..jobs {
            let result = results[i % workers]
                .recv()
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            output.write_all(&result.bytes)?;
            stored += result.bytes.len() as u64;
        }
        output.flush()?;
        Ok(stored)
    });
    for i in 0..jobs {
        let source = if kind == "binary" || (kind == "mixed" && i % 4 != 0) {
            &binary
        } else {
            &text
        };
        // Stable body routing gives every worker the same adaptive policy and
        // keeps the probe decisions independent of the pool size.
        let stream = (i % 64) as u64;
        let body = if kind == "small" { i as u64 } else { stream };
        let before = Instant::now();
        senders[i % workers].send((body, source.clone()))?;
        waits.push(u64::try_from(before.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
    drop(senders);
    let attempted: u64 = threads
        .into_iter()
        .map(|t| t.join().unwrap().unwrap())
        .sum();
    let stored = consumer.join().unwrap()?;
    let elapsed = start.elapsed().as_secs_f64();
    waits.sort_unstable();
    println!(
        "{{\"kind\":\"{kind}\",\"workers\":{workers},\"level\":{level},\"sink\":\"{}\",\"input_mib\":{mib},\"seconds\":{elapsed:.6},\"mib_s\":{:.2},\"admission_p95_us\":{:.2},\"admission_p99_us\":{:.2},\"attempted_bytes\":{attempted},\"stored_bytes\":{stored}}}",
        if disk { "file" } else { "null" },
        mib as f64 / elapsed,
        waits[waits.len() * 95 / 100] as f64 / 1000.0,
        waits[waits.len() * 99 / 100] as f64 / 1000.0
    );
    Ok(())
}
