//! Bounded definite-length ASN.1 shared by SPNEGO and Kerberos.
use crate::limits::MAX_ASN1_CHILDREN;
use crate::{Context, MAX_DEPTH, MAX_FIELDS};

pub(super) const TAG_NUMBER_MASK: u8 = 0x1f;
const CONSTRUCTED_BIT: u8 = 0x20;
const HIGH_BIT: u8 = 0x80;
const LOW_BITS: u8 = 0x7f;
const HEADER_BYTES: usize = 2;
const BYTE_RADIX: usize = 256;
const OID_FIRST_ARC_BASE: u64 = 40;
const OID_MAXIMUM_FIRST_ARC: u64 = 2;

pub(super) mod tag {
    pub const INTEGER: u8 = 0x02;
    pub const BIT_STRING: u8 = 0x03;
    pub const OCTET_STRING: u8 = 0x04;
    pub const OBJECT_IDENTIFIER: u8 = 0x06;
    pub const ENUMERATED: u8 = 0x0a;
    pub const UTF8_STRING: u8 = 0x0c;
    pub const GENERALIZED_TIME: u8 = 0x18;
    pub const GENERAL_STRING: u8 = 0x1b;
    pub const UNIVERSAL_STRING: u8 = 0x1c;
    pub const SEQUENCE: u8 = 0x30;
    pub const GSS_INITIAL_CONTEXT: u8 = 0x60;
    pub const KERBEROS_TICKET: u8 = 0x61;
    pub const KERBEROS_AP_REQUEST: u8 = 0x6e;
    pub const KERBEROS_AP_REPLY: u8 = 0x6f;
    pub const KERBEROS_ERROR: u8 = 0x7e;
}

pub(super) struct Tlv<'a> {
    pub(super) tag: u8,
    pub(super) bytes: &'a [u8],
    pub(super) rest: &'a [u8],
}
pub(super) fn tlv(bytes: &[u8]) -> Option<Tlv<'_>> {
    let tag = *bytes.first()?;
    if tag & TAG_NUMBER_MASK == TAG_NUMBER_MASK {
        return None;
    }
    let first = *bytes.get(1)?;
    let mut at = HEADER_BYTES;
    let length = if first < HIGH_BIT {
        usize::from(first)
    } else {
        let count = usize::from(first & LOW_BITS);
        if count == 0 || count > size_of::<u32>() || bytes.get(HEADER_BYTES) == Some(&0) {
            return None;
        }
        let mut size = 0usize;
        for byte in bytes.get(at..at + count)? {
            size = size
                .checked_mul(BYTE_RADIX)?
                .checked_add(usize::from(*byte))?;
        }
        at += count;
        if size < usize::from(HIGH_BIT) {
            return None;
        }
        size
    };
    let end = at.checked_add(length)?;
    Some(Tlv {
        tag,
        bytes: bytes.get(at..end)?,
        rest: bytes.get(end..)?,
    })
}
pub(super) fn children(bytes: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut rest = bytes;
    let mut result = vec![];
    while !rest.is_empty() {
        if result.len() >= MAX_ASN1_CHILDREN {
            return None;
        }
        let item = tlv(rest)?;
        rest = item.rest;
        result.push(item);
    }
    Some(result)
}
pub(super) fn validate(
    bytes: &[u8],
    depth: usize,
    nodes: &mut usize,
    context: &mut Context,
) -> bool {
    if depth > MAX_DEPTH || *nodes >= MAX_FIELDS || context.is_canceled() {
        return false;
    }
    let Some(items) = children(bytes) else {
        return false;
    };
    for item in items {
        *nodes += 1;
        if *nodes > MAX_FIELDS
            || item.tag & CONSTRUCTED_BIT != 0 && !validate(item.bytes, depth + 1, nodes, context)
        {
            return false;
        }
    }
    true
}
pub(super) fn oid(bytes: &[u8]) -> Option<String> {
    let mut arcs = vec![];
    let mut number = 0u64;
    let mut pending = false;
    for byte in bytes {
        number = number
            .checked_mul(u64::from(HIGH_BIT))?
            .checked_add(u64::from(byte & LOW_BITS))?;
        pending = byte & HIGH_BIT != 0;
        if !pending {
            if arcs.is_empty() {
                let first = (number / OID_FIRST_ARC_BASE).min(OID_MAXIMUM_FIRST_ARC);
                arcs.push(first);
                arcs.push(number - first * OID_FIRST_ARC_BASE);
            } else {
                arcs.push(number);
            }
            number = 0;
        }
    }
    if pending || arcs.is_empty() {
        return None;
    }
    Some(
        arcs.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}
pub(super) fn int(bytes: &[u8]) -> Option<i64> {
    let item = tlv(bytes)?;
    if item.tag != tag::INTEGER && item.tag != tag::ENUMERATED
        || !item.rest.is_empty()
        || item.bytes.is_empty()
        || item.bytes.len() > size_of::<i64>()
    {
        return None;
    }
    let mut value = if item.bytes[0] & HIGH_BIT != 0 {
        -1i64
    } else {
        0
    };
    for b in item.bytes {
        value = (value << u8::BITS) | i64::from(*b);
    }
    Some(value)
}
pub(super) fn context_field<'a>(items: &'a [Tlv<'a>], tag: u8) -> Option<&'a [u8]> {
    let mut matches = items.iter().filter(|v| v.tag == tag);
    let value = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(value.bytes)
}
