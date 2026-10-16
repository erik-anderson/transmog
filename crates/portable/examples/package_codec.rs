//! Developer qualification of ZIP/compression composition; not a product CLI.
use std::{
    error::Error,
    fs::File,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use transmog_capture::{
    CaptureEncoding, CaptureLimits, CapturePassword, CaptureRecord, CaptureRecordKind,
    CaptureWriter,
    artifact::{ArtifactReader, ArtifactWriter},
};
struct Counted {
    file: File,
    read: Arc<AtomicU64>,
}
impl Read for Counted {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(output)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}
impl Seek for Counted {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.file.seek(from)
    }
}
fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn password(encrypted: bool) -> Option<CapturePassword> {
    encrypted.then(|| CapturePassword::new("synthetic-benchmark-only".into()))
}
fn generate(path: &str, encrypted: bool) -> Result<(), Box<dyn Error>> {
    let mut writer = CaptureWriter::with_encoding(
        BufWriter::new(File::create(path)?),
        CaptureLimits::default(),
        &CaptureEncoding {
            password: password(encrypted),
        },
    )?;
    let mut random = 0x1a2b_3c4d_5e6f_7081_u64;
    for sequence in 0..256 {
        let mut bytes = vec![0; 128 * 1024];
        for byte in &mut bytes {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            *byte = random.to_le_bytes()[0];
        }
        writer.append(&CaptureRecord {
            sequence,
            exchange_id: 1,
            kind: CaptureRecordKind::BodySegment {
                boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
                byte_count: bytes.len(),
                bytes: Some(bytes),
                truncated: false,
            },
        })?;
    }
    writer.seal()?;
    writer.into_inner()?.flush()?;
    Ok(())
}
fn probe_entries(
    archive: &mut zip::ZipArchive<ArtifactReader<Counted>>,
    read: &AtomicU64,
) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let mut probes = vec![];
    for i in 0..archive.len() {
        let member = archive.by_index(i)?;
        let size = member.size();
        let stored = member.compression() == zip::CompressionMethod::Stored;
        drop(member);
        let count = usize::try_from(size.min(65536))?;
        let skip = size.saturating_sub(count as u64) / 2;
        let before = read.load(Ordering::Relaxed);
        let start = Instant::now();
        let mut bytes = vec![0; count];
        if stored {
            let mut member = archive.by_index_seek(i)?;
            member.seek(SeekFrom::Start(skip))?;
            member.read_exact(&mut bytes)?;
        } else {
            let mut member = archive.by_index(i)?;
            io::copy(&mut (&mut member).take(skip), &mut io::sink())?;
            member.read_exact(&mut bytes)?;
        }
        probes.push(serde_json::json!({"entry":i,"bytes":count,"milliseconds":milliseconds(start),"physicalReadBytes":read.load(Ordering::Relaxed)-before}));
    }
    Ok(probes)
}
fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) == Some("generate") {
        let encrypted = args.get(3).map(String::as_str) == Some("encrypted");
        generate(&args[2], encrypted)?;
        return Ok(());
    }
    let input = &args[1];
    let output = &args[2];
    let policy = &args[3];
    let encrypted = args.get(4).map(String::as_str) == Some("encrypted");
    let mut zip = zip::ZipArchive::new(File::open(input)?)?;
    let mut boundaries = vec![];
    for i in 0..zip.len() {
        let member = zip.by_index(i)?;
        boundaries.push(
            member
                .data_start()
                .ok_or_else(|| io::Error::other("ZIP member has no data offset"))?
                + member.compressed_size(),
        );
    }
    boundaries.sort_unstable();
    let length = std::fs::metadata(input)?.len();
    let begin = Instant::now();
    let key_begin = Instant::now();
    let mut writer = ArtifactWriter::new(
        BufWriter::new(File::create(output)?),
        "application/vnd.transmog.case+zip",
        &CaptureEncoding {
            password: password(encrypted),
        },
        policy != "raw",
    )?;
    let writer_initialization_ms = milliseconds(key_begin);
    let mut source = File::open(input)?;
    let mut position = 0;
    let mut buffer = vec![0; 128 * 1024];
    while position < length {
        let next = if policy == "boundary" {
            boundaries
                .iter()
                .copied()
                .find(|b| *b > position)
                .unwrap_or(length)
        } else {
            length
        };
        let count = usize::try_from((next - position).min(buffer.len() as u64))?;
        source.read_exact(&mut buffer[..count])?;
        writer.write_all(&buffer[..count])?;
        position += count as u64;
        if policy == "boundary" && position == next {
            writer.member_boundary()?;
        }
    }
    let (mut output_file, _) = writer.finish()?;
    output_file.flush()?;
    output_file.get_ref().sync_all()?;
    drop(output_file);
    let pack_ms = milliseconds(begin);
    let read = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let mut reader = ArtifactReader::new(
        Counted {
            file: File::open(output)?,
            read: read.clone(),
        },
        password(encrypted).as_ref(),
    )?;
    let index_open_ms = milliseconds(start);
    let start = Instant::now();
    reader.verify(|| false)?;
    let stream_verify_ms = milliseconds(start);
    let start = Instant::now();
    let mut archive = zip::ZipArchive::new(reader)?;
    let zip_open_ms = milliseconds(start);
    let start = Instant::now();
    for i in 0..archive.len() {
        io::copy(&mut archive.by_index(i)?, &mut io::sink())?;
    }
    let zip_verify_ms = milliseconds(start);
    let probes = probe_entries(&mut archive, &read)?;
    println!(
        "{}",
        serde_json::json!({"policy":policy,"encrypted":encrypted,"innerBytes":length,"outerBytes":std::fs::metadata(output)?.len(),"packMilliseconds":pack_ms,"writerInitializationMilliseconds":writer_initialization_ms,"indexOpenMilliseconds":index_open_ms,"streamVerifyMilliseconds":stream_verify_ms,"zipOpenMilliseconds":zip_open_ms,"zipVerifyMilliseconds":zip_verify_ms,"probes":probes})
    );
    Ok(())
}
