//! Bind decoded representations to their pending original body and input grant.
use super::{Pending, fail, protocol};
use std::collections::BTreeMap;
use transmog_extensions::{Failure, FailureKind, HttpDecodeResult, decimal, valid_id};

pub(super) fn resume(
    token: &str,
    result: &HttpDecodeResult,
    pending: &mut Pending,
    grants: &mut BTreeMap<String, u64>,
    counters: (&mut usize, &mut usize),
    limit: usize,
) -> Result<(), Failure> {
    let (remaining, continuations) = counters;
    if pending.token.as_deref() != Some(token) {
        return Err(protocol());
    }
    let original = pending.decode.as_deref().ok_or_else(protocol)?;
    let length = grants.get(original).copied().ok_or_else(protocol)?;
    *continuations += 1;
    if *continuations > 512 {
        return Err(fail(FailureKind::Limit, "Too many parser continuations"));
    }
    // Encoded work and subsequent decoded byte pages share the operation budget.
    *remaining = remaining
        .checked_sub(usize::try_from(length).map_err(|_| protocol())?)
        .ok_or_else(|| fail(FailureKind::Limit, "HTTP decode input budget exhausted"))?;
    let HttpDecodeResult::Ready { decoded } = result else {
        return Ok(());
    };
    let output = decimal(&decoded.body.length).map_err(|_| protocol())?;
    if decoded.parent.source_id != original
        || decoded.parent.offset != "0"
        || decimal(&decoded.parent.length).ok() != Some(length)
        || decoded.body.availability != "complete"
        || output > limit as u64
        || !valid_id(&decoded.body.source_id)
        || decoded.codings.len() > 8
        || decoded
            .codings
            .iter()
            .any(|coding| !matches!(coding.as_str(), "gzip" | "deflate" | "br" | "zstd"))
    {
        return Err(protocol());
    }
    if decoded.codings.is_empty() {
        if decoded.body.source_id != original || output != length {
            return Err(protocol());
        }
        return Ok(());
    }
    if !decoded.body.source_id.starts_with("decoded-")
        || pending
            .decoded
            .get(&decoded.body.source_id)
            .is_some_and(|prior| prior != decoded)
        || grants
            .get(&decoded.body.source_id)
            .is_some_and(|prior| *prior != output)
        || (!grants.contains_key(&decoded.body.source_id) && grants.len() >= 4096)
    {
        return Err(protocol());
    }
    grants.insert(decoded.body.source_id.clone(), output);
    pending
        .decoded
        .insert(decoded.body.source_id.clone(), decoded.clone());
    Ok(())
}
