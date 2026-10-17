//! Bounded Stored ZIP/ZIP64, checked before any content is exposed.
use crate::{Error, invalid};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
};

pub(super) const MAX_DIRECTORY: u64 = 16 * 1024 * 1024;
const MAX_MEMBERS: u64 = 16_385;
#[derive(Clone, Debug)]
pub(super) struct Member {
    pub offset: u64,
    pub length: u64,
    pub crc: u32,
}
fn u16b(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32b(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn u64b(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
fn at(
    input: &mut (impl Read + Seek + ?Sized),
    offset: u64,
    length: usize,
) -> Result<Vec<u8>, Error> {
    input.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn name(bytes: &[u8]) -> Result<String, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Invalid ZIP member name"))?;
    if text != "manifest.json"
        && !text
            .strip_prefix("objects/sha256/")
            .is_some_and(crate::valid_digest)
    {
        return Err(invalid("Unexpected or unsafe ZIP member"));
    }
    Ok(text.into())
}
fn extra(bytes: &[u8], need: &[bool]) -> Result<Vec<u64>, Error> {
    let mut rest = bytes;
    let mut zip64 = None;
    while !rest.is_empty() {
        if rest.len() < 4 {
            return Err(invalid("Truncated ZIP extra field"));
        }
        let length = u16b(rest, 2) as usize;
        if length + 4 > rest.len() {
            return Err(invalid("Truncated ZIP extra field"));
        }
        if u16b(rest, 0) != 1 {
            return Err(Error::Unsupported(
                "ZIP metadata extra fields outside the ZIP64 profile".into(),
            ));
        }
        if u16b(rest, 0) == 1 && zip64.replace(&rest[4..4 + length]).is_some() {
            return Err(invalid("Duplicate ZIP64 field"));
        }
        rest = &rest[4 + length..];
    }
    let mut data = zip64.unwrap_or_default();
    let mut result = vec![];
    for required in need {
        if *required {
            if data.len() < 8 {
                return Err(invalid("Missing ZIP64 size/offset"));
            }
            result.push(u64b(data, 0));
            data = &data[8..];
        } else {
            result.push(0);
        }
    }
    if !data.is_empty() {
        return Err(invalid("Unexpected ZIP64 fields"));
    }
    Ok(result)
}
fn directory(input: &mut (impl Read + Seek + ?Sized)) -> Result<(Vec<u8>, u64, u64), Error> {
    let length = input.seek(SeekFrom::End(0))?;
    if length < 22 {
        return Err(invalid("Incomplete ZIP"));
    }
    let tail_len = length.min(65_557) as usize;
    let tail = at(input, length - tail_len as u64, tail_len)?;
    let end = (0..=tail_len - 22)
        .rev()
        .find(|i| {
            u32b(&tail, *i) == 0x0605_4b50 && *i + 22 + u16b(&tail, *i + 20) as usize == tail.len()
        })
        .ok_or_else(|| invalid("Missing ZIP completion"))?;
    let eocd = &tail[end..];
    let end_offset = length - tail_len as u64 + end as u64;
    if u16b(eocd, 4) != 0 || u16b(eocd, 6) != 0 || u16b(eocd, 8) != u16b(eocd, 10) {
        return Err(invalid("Multi-volume ZIP is unsupported"));
    }
    let mut count = u64::from(u16b(eocd, 10));
    let mut size = u64::from(u32b(eocd, 12));
    let mut offset = u64::from(u32b(eocd, 16));
    let mut directory_end = end_offset;
    if count == 65535 || size == u64::from(u32::MAX) || offset == u64::from(u32::MAX) {
        let locator_at = end_offset
            .checked_sub(20)
            .ok_or_else(|| invalid("Missing ZIP64 locator"))?;
        let locator = at(input, locator_at, 20)?;
        if u32b(&locator, 0) != 0x0706_4b50 || u32b(&locator, 4) != 0 || u32b(&locator, 16) != 1 {
            return Err(invalid("Invalid ZIP64 locator"));
        }
        let record_at = u64b(&locator, 8);
        let record = at(input, record_at, 56)?;
        if u32b(&record, 0) != 0x0606_4b50
            || u64b(&record, 4) != 44
            || record_at.checked_add(56) != Some(locator_at)
            || u32b(&record, 16) != 0
            || u32b(&record, 20) != 0
            || u64b(&record, 24) != u64b(&record, 32)
        {
            return Err(invalid("Invalid ZIP64 completion"));
        }
        count = u64b(&record, 32);
        size = u64b(&record, 40);
        offset = u64b(&record, 48);
        if (u16b(eocd, 10) != u16::MAX && u64::from(u16b(eocd, 10)) != count)
            || (u32b(eocd, 12) != u32::MAX && u64::from(u32b(eocd, 12)) != size)
            || (u32b(eocd, 16) != u32::MAX && u64::from(u32b(eocd, 16)) != offset)
        {
            return Err(invalid("ZIP32/ZIP64 completion disagrees"));
        }
        directory_end = record_at;
    }
    if count == 0
        || count > MAX_MEMBERS
        || size > MAX_DIRECTORY
        || offset.checked_add(size) != Some(directory_end)
    {
        return Err(invalid("ZIP directory exceeds bounds or completion"));
    }
    let directory = at(
        input,
        offset,
        usize::try_from(size).map_err(|_| invalid("ZIP directory length overflow"))?,
    )?;
    Ok((directory, offset, count))
}
fn check_encoding(head: &[u8]) -> Result<(), Error> {
    let flags = u16b(head, 8);
    if u32b(head, 0) != 0x0201_4b50
        || u16b(head, 6) > 45
        || flags & !0x0808 != 0
        || u16b(head, 10) != 0
        || u16b(head, 34) != 0
    {
        return Err(invalid("Unsupported ZIP member encoding"));
    }
    let mode = u32b(head, 38) >> 16;
    if (mode & 0o170_000 != 0 && mode & 0o170_000 != 0o100_000) || u32b(head, 38) & 0x410 != 0 {
        return Err(invalid("ZIP links/directories are forbidden"));
    }
    Ok(())
}
pub(super) fn read(
    input: &mut (impl Read + Seek + ?Sized),
    canceled: &std::sync::atomic::AtomicBool,
) -> Result<BTreeMap<String, Member>, Error> {
    let (directory, offset, count) = directory(input)?;
    let mut pos = 0;
    let mut entries = BTreeMap::new();
    let mut ranges = vec![];
    for _ in 0..count {
        if canceled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        if directory.len() - pos < 46 {
            return Err(invalid("Truncated ZIP directory"));
        }
        let head = &directory[pos..pos + 46];
        check_encoding(head)?;
        let names = u16b(head, 28) as usize;
        let extras = u16b(head, 30) as usize;
        let comment = u16b(head, 32) as usize;
        let next = pos
            .checked_add(46 + names + extras + comment)
            .ok_or_else(|| invalid("ZIP length overflow"))?;
        if next > directory.len() {
            return Err(invalid("Truncated ZIP member"));
        }
        let member_name = name(&directory[pos + 46..pos + 46 + names])?;
        let needs = [
            u32b(head, 24) == u32::MAX,
            u32b(head, 20) == u32::MAX,
            u32b(head, 42) == u32::MAX,
        ];
        if needs.iter().any(|n| *n) && u16b(head, 6) < 45 {
            return Err(invalid("ZIP64 requires version 45"));
        }
        let large = extra(
            &directory[pos + 46 + names..pos + 46 + names + extras],
            &needs,
        )?;
        let plain = if needs[0] {
            large[0]
        } else {
            u64::from(u32b(head, 24))
        };
        let stored = if needs[1] {
            large[1]
        } else {
            u64::from(u32b(head, 20))
        };
        let local_at = if needs[2] {
            large[2]
        } else {
            u64::from(u32b(head, 42))
        };
        if stored != plain || local_at >= offset {
            return Err(invalid("Invalid Stored ZIP length"));
        }
        let crc = u32b(head, 16);
        let (body, end) = local_member(
            input,
            head,
            &directory[pos + 46..pos + 46 + names],
            plain,
            local_at,
            offset,
        )?;
        if entries
            .insert(
                member_name,
                Member {
                    offset: body,
                    length: plain,
                    crc,
                },
            )
            .is_some()
        {
            return Err(invalid("Duplicate ZIP member"));
        }
        ranges.push((local_at, end));
        pos = next;
    }
    if pos != directory.len() {
        return Err(invalid("Unlisted ZIP directory data"));
    }
    ranges.sort_unstable();
    let mut end = 0;
    for (start, stop) in ranges {
        if start != end || stop > offset {
            return Err(invalid("Overlapping or unlisted ZIP data"));
        }
        end = stop;
    }
    if end != offset {
        return Err(invalid("Unlisted ZIP content"));
    }
    Ok(entries)
}

fn local_member(
    input: &mut (impl Read + Seek + ?Sized),
    head: &[u8],
    name: &[u8],
    plain: u64,
    local_at: u64,
    offset: u64,
) -> Result<(u64, u64), Error> {
    let names = name.len();
    let stored = plain;
    let flags = u16b(head, 8);
    let crc = u32b(head, 16);
    let local = at(input, local_at, 30)?;
    let local_names = u16b(&local, 26) as usize;
    let local_extras = u16b(&local, 28) as usize;
    if u32b(&local, 0) != 0x0403_4b50
        || u16b(&local, 4) != u16b(head, 6)
        || u16b(&local, 6) != flags
        || u16b(&local, 8) != 0
        || local_names != names
        || local[10..14] != head[12..16]
    {
        return Err(invalid("ZIP local/central mismatch"));
    }
    let local_fields = at(input, local_at + 30, local_names + local_extras)?;
    if local_fields[..names] != *name {
        return Err(invalid("ZIP member names disagree"));
    }
    let local_needs = [u32b(&local, 22) == u32::MAX, u32b(&local, 18) == u32::MAX];
    let local_large = extra(&local_fields[names..], &local_needs)?;
    let local_plain = if local_needs[0] {
        local_large[0]
    } else {
        u64::from(u32b(&local, 22))
    };
    let local_stored = if local_needs[1] {
        local_large[1]
    } else {
        u64::from(u32b(&local, 18))
    };
    let body = local_at
        .checked_add(30 + local_fields.len() as u64)
        .ok_or_else(|| invalid("ZIP offset overflow"))?;
    let mut end = body
        .checked_add(stored)
        .ok_or_else(|| invalid("ZIP length overflow"))?;
    if end > offset {
        return Err(invalid("ZIP member overlaps directory"));
    }

    if flags & 8 == 0 {
        if local_plain != plain || local_stored != stored || u32b(&local, 14) != crc {
            return Err(invalid("ZIP local sizes/CRC disagree"));
        }
    } else {
        if (local_plain != 0 && local_plain != plain)
            || (local_stored != 0 && local_stored != stored)
            || (u32b(&local, 14) != 0 && u32b(&local, 14) != crc)
        {
            return Err(invalid("Invalid ZIP streaming placeholders"));
        }
        let is64 = local_needs.iter().any(|v| *v) || plain >= u64::from(u32::MAX);
        end += descriptor_length(input, end, is64, crc, plain, offset)?;
    }
    Ok((body, end))
}

fn descriptor_length(
    input: &mut (impl Read + Seek + ?Sized),
    position: u64,
    is64: bool,
    crc: u32,
    length: u64,
    directory: u64,
) -> Result<u64, Error> {
    let base = if is64 { 20 } else { 12 };
    let bytes = at(input, position, base + 4)?;
    let matches = |skip: usize| {
        let sizes = if is64 {
            [u64b(&bytes, skip + 4), u64b(&bytes, skip + 12)]
        } else {
            [
                u64::from(u32b(&bytes, skip + 4)),
                u64::from(u32b(&bytes, skip + 8)),
            ]
        };
        u32b(&bytes, skip) == crc
            && sizes == [length, length]
            && position
                .checked_add((base + skip) as u64)
                .is_some_and(|end| end <= directory)
    };
    if u32b(&bytes, 0) == 0x0807_4b50 && matches(4) {
        return Ok((base + 4) as u64);
    }
    if matches(0) {
        return Ok(base as u64);
    }
    Err(invalid("ZIP data descriptor disagrees"))
}
pub(super) struct Writer<W> {
    output: W,
    offset: u64,
    directory: Vec<u8>,
    count: u64,
    modified: u32,
}
impl<W: Write> Writer<W> {
    pub fn new(output: W) -> Self {
        Self {
            output,
            offset: 0,
            directory: vec![],
            count: 0,
            modified: dos_timestamp(time::OffsetDateTime::now_utc()),
        }
    }
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.output.write_all(bytes)?;
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| invalid("ZIP length overflow"))?;
        Ok(())
    }
    pub fn member(
        &mut self,
        member_name: &str,
        input: &mut dyn Read,
        expected: u64,
        canceled: &std::sync::atomic::AtomicBool,
    ) -> Result<(), Error> {
        name(member_name.as_bytes())?;
        let start = self.offset;
        let mut head = vec![];
        put32(&mut head, 0x0403_4b50);
        put16(&mut head, 45);
        put16(&mut head, 0x808);
        put16(&mut head, 0);
        put32(&mut head, self.modified);
        put32(&mut head, 0);
        put32(&mut head, u32::MAX);
        put32(&mut head, u32::MAX);
        put16(
            &mut head,
            u16::try_from(member_name.len()).map_err(|_| invalid("ZIP member name too long"))?,
        );
        put16(&mut head, 20);
        head.extend_from_slice(member_name.as_bytes());
        put16(&mut head, 1);
        put16(&mut head, 16);
        head.extend_from_slice(&[0; 16]);
        self.bytes(&head)?;
        let mut crc = crc32fast::Hasher::new();
        let mut buffer = vec![0; 128 * 1024];
        let mut count = 0_u64;
        loop {
            if canceled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            count = count
                .checked_add(n as u64)
                .ok_or_else(|| invalid("ZIP length overflow"))?;
            if count > expected {
                return Err(invalid("Object grew during packaging"));
            }
            crc.update(&buffer[..n]);
            self.bytes(&buffer[..n])?;
        }
        if count != expected {
            return Err(invalid("Object length changed during packaging"));
        }
        let crc = crc.finalize();
        let mut descriptor = vec![];
        put32(&mut descriptor, 0x0807_4b50);
        put32(&mut descriptor, crc);
        put64(&mut descriptor, count);
        put64(&mut descriptor, count);
        self.bytes(&descriptor)?;
        let d = &mut self.directory;
        put32(d, 0x0201_4b50);
        put16(d, 0x032d);
        put16(d, 45);
        put16(d, 0x808);
        put16(d, 0);
        put32(d, self.modified);
        put32(d, crc);
        put32(d, u32::MAX);
        put32(d, u32::MAX);
        put16(
            d,
            u16::try_from(member_name.len()).map_err(|_| invalid("ZIP member name too long"))?,
        );
        put16(d, 28);
        put16(d, 0);
        put16(d, 0);
        put16(d, 0);
        put32(d, 0o100_600 << 16);
        put32(d, u32::MAX);
        d.extend_from_slice(member_name.as_bytes());
        put16(d, 1);
        put16(d, 24);
        put64(d, count);
        put64(d, count);
        put64(d, start);
        self.count += 1;
        if self.count > MAX_MEMBERS || d.len() as u64 > MAX_DIRECTORY {
            return Err(invalid("ZIP directory limit"));
        }
        Ok(())
    }
    pub fn boundary(
        &mut self,
        action: &mut impl FnMut(&mut W) -> Result<(), Error>,
    ) -> Result<(), Error> {
        action(&mut self.output)
    }
    pub fn finish(mut self) -> Result<W, Error> {
        let start = self.offset;
        let directory = std::mem::take(&mut self.directory);
        self.bytes(&directory)?;
        let record = self.offset;
        let mut end = vec![];
        put32(&mut end, 0x0606_4b50);
        put64(&mut end, 44);
        put16(&mut end, 45);
        put16(&mut end, 45);
        put32(&mut end, 0);
        put32(&mut end, 0);
        put64(&mut end, self.count);
        put64(&mut end, self.count);
        put64(&mut end, directory.len() as u64);
        put64(&mut end, start);
        put32(&mut end, 0x0706_4b50);
        put32(&mut end, 0);
        put64(&mut end, record);
        put32(&mut end, 1);
        put32(&mut end, 0x0605_4b50);
        put16(&mut end, 0);
        put16(&mut end, 0);
        put16(&mut end, 65535);
        put16(&mut end, 65535);
        put32(&mut end, u32::MAX);
        put32(&mut end, u32::MAX);
        put16(&mut end, 0);
        self.bytes(&end)?;
        self.output.flush()?;
        Ok(self.output)
    }
}
// ZIP's DOS clock has no time-zone field and two-second precision. Use UTC
// export time for newly created members, not an invented source modification time.
fn dos_timestamp(at: time::OffsetDateTime) -> u32 {
    let year = at.year().clamp(1980, 2107).cast_unsigned();
    let date = ((year - 1980) << 9) | (u32::from(u8::from(at.month())) << 5) | u32::from(at.day());
    let clock =
        (u32::from(at.hour()) << 11) | (u32::from(at.minute()) << 5) | u32::from(at.second() / 2);
    (date << 16) | clock
}
fn put16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn put32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn put64(out: &mut Vec<u8>, n: u64) {
    out.extend_from_slice(&n.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn unsigned_descriptor_crc_may_equal_the_optional_signature() {
        let body = [0xac, 0x0a, 0x7a, 0xd5];
        assert_eq!(crc32fast::hash(&body), 0x0807_4b50);
        let name = format!("objects/sha256/{}", crate::sha256(&body));
        let flag = std::sync::atomic::AtomicBool::new(false);
        let mut writer = Writer::new(Vec::new());
        writer
            .member(&name, &mut body.as_slice(), body.len() as u64, &flag)
            .unwrap();
        let mut bytes = writer.finish().unwrap();
        let index = read(&mut Cursor::new(&bytes), &flag).unwrap();
        let start = usize::try_from(index[&name].offset).unwrap() + body.len();
        bytes.drain(start..start + 4);
        let record = bytes.len() - 98;
        let central = u64b(&bytes, record + 48) - 4;
        bytes[record + 48..record + 56].copy_from_slice(&central.to_le_bytes());
        bytes[record + 64..record + 72].copy_from_slice(&(record as u64).to_le_bytes());
        assert_eq!(
            read(&mut Cursor::new(&bytes), &flag).unwrap()[&name].length,
            4
        );
        let mut standard = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut actual = vec![];
        standard
            .by_index(0)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, body);
    }
    #[test]
    fn unicode_path_alias_cannot_override_a_canonical_digest_member() {
        let name = format!("objects/sha256/{}", crate::sha256(&[0]));
        let flag = std::sync::atomic::AtomicBool::new(false);
        let mut writer = Writer::new(Vec::new());
        writer.member(&name, &mut &[0_u8][..], 1, &flag).unwrap();
        let original = writer.finish().unwrap();
        let index = read(&mut Cursor::new(&original), &flag).unwrap();
        let body = usize::try_from(index[&name].offset).unwrap();
        let record = original.len() - 98;
        let central = usize::try_from(u64b(&original, record + 48)).unwrap();
        let target = b"../../alias";
        let mut alias = vec![];
        put16(&mut alias, 0x7075);
        put16(&mut alias, u16::try_from(5 + target.len()).unwrap());
        alias.push(1);
        put32(&mut alias, crc32fast::hash(name.as_bytes()));
        alias.extend_from_slice(target);
        let mut local = original[..body].to_vec();
        local[6..8].copy_from_slice(&8_u16.to_le_bytes());
        let size = u16b(&local, 28) + u16::try_from(alias.len()).unwrap();
        local[28..30].copy_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&alias);
        let mut directory = original[central..record].to_vec();
        directory[8..10].copy_from_slice(&8_u16.to_le_bytes());
        let size = u16b(&directory, 30) + u16::try_from(alias.len()).unwrap();
        directory[30..32].copy_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&alias);
        let mut footer = original[record..].to_vec();
        footer[40..48].copy_from_slice(&(directory.len() as u64).to_le_bytes());
        footer[48..56].copy_from_slice(&((central + alias.len()) as u64).to_le_bytes());
        footer[64..72].copy_from_slice(&((record + 2 * alias.len()) as u64).to_le_bytes());
        let mut bytes = local;
        bytes.extend_from_slice(&original[body..central]);
        bytes.extend_from_slice(&directory);
        bytes.extend_from_slice(&footer);
        let mut standard = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(standard.by_index(0).unwrap().name().unwrap(), "../../alias");
        assert!(matches!(
            read(&mut Cursor::new(&bytes), &flag),
            Err(Error::Unsupported(_))
        ));
    }
    struct Sparse {
        prefix: Vec<u8>,
        tail: Vec<u8>,
        tail_at: u64,
        position: u64,
        read: usize,
    }
    impl Read for Sparse {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let length = self.tail_at + self.tail.len() as u64;
            let count = output
                .len()
                .min(usize::try_from(length.saturating_sub(self.position)).unwrap_or(usize::MAX));
            output[..count].fill(0);
            for (at, bytes) in [(0, &self.prefix), (self.tail_at, &self.tail)] {
                let start = self.position.max(at);
                let end = (self.position + count as u64).min(at + bytes.len() as u64);
                if start < end {
                    let target = usize::try_from(start - self.position).unwrap();
                    let source = usize::try_from(start - at).unwrap();
                    let length = usize::try_from(end - start).unwrap();
                    output[target..target + length]
                        .copy_from_slice(&bytes[source..source + length]);
                }
            }
            self.position += count as u64;
            self.read += count;
            Ok(count)
        }
    }
    impl Seek for Sparse {
        fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
            let next = match from {
                SeekFrom::Start(n) => i128::from(n),
                SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
                SeekFrom::End(n) => {
                    i128::from(self.tail_at + self.tail.len() as u64) + i128::from(n)
                }
            };
            self.position =
                u64::try_from(next).map_err(|_| std::io::Error::other("Invalid sparse seek"))?;
            Ok(self.position)
        }
    }
    #[test]
    fn zip64_metadata_above_four_gib_never_allocates_or_reads_the_declared_payload() {
        let name = format!("objects/sha256/{}", "a".repeat(64));
        let flag = std::sync::atomic::AtomicBool::new(false);
        let mut writer = Writer::new(Vec::new());
        writer.member(&name, &mut &[0_u8][..], 1, &flag).unwrap();
        let bytes = writer.finish().unwrap();
        let index = read(&mut Cursor::new(&bytes), &flag).unwrap();
        let body = usize::try_from(index[&name].offset).unwrap();
        let payload = u64::from(u32::MAX) + 1024;
        let mut tail = bytes[body + 1..].to_vec();
        tail[4..8].copy_from_slice(&0_u32.to_le_bytes());
        tail[8..16].copy_from_slice(&payload.to_le_bytes());
        tail[16..24].copy_from_slice(&payload.to_le_bytes());
        tail[40..44].copy_from_slice(&0_u32.to_le_bytes());
        let extra = 24 + 46 + name.len() + 4;
        tail[extra..extra + 8].copy_from_slice(&payload.to_le_bytes());
        tail[extra + 8..extra + 16].copy_from_slice(&payload.to_le_bytes());
        let record = 24 + 46 + name.len() + 28;
        let tail_at = body as u64 + payload;
        tail[record + 48..record + 56].copy_from_slice(&(tail_at + 24).to_le_bytes());
        tail[record + 64..record + 72].copy_from_slice(&(tail_at + record as u64).to_le_bytes());
        let mut sparse = Sparse {
            prefix: bytes[..body].to_vec(),
            tail,
            tail_at,
            position: 0,
            read: 0,
        };
        let result = read(&mut sparse, &flag).unwrap();
        assert_eq!(result[&name].length, payload);
        assert!(sparse.read < 256 * 1024);
    }
}
