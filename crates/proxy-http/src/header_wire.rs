//! Bridge Hyper's optional HTTP/1 wire metadata into protocol-neutral fields.
use http::{Extensions, HeaderMap};
use std::collections::HashSet;
use transmog_core::{HeaderBlock, HeaderError, HeaderField};

/// Converts headers while retaining original spelling and interleaved duplicates
/// when the HTTP/1 parser recorded them. Stale metadata cannot add or duplicate
/// fields; edits absent from the metadata are appended in their current order.
/// # Errors
/// Returns invalid canonical header syntax.
pub fn header_block_from_hyper(
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Result<HeaderBlock, HeaderError> {
    let cases = extensions.get::<hyper::ext::HeaderCaseMap>();
    let order = extensions.get::<hyper::ext::OriginalHeaderOrder>();
    let preferred = order
        .into_iter()
        .flat_map(hyper::ext::OriginalHeaderOrder::get_in_order);
    let fallback = headers
        .keys()
        .flat_map(|name| (0..headers.get_all(name).iter().count()).map(move |index| (name, index)));
    let mut seen = HashSet::new();
    let mut fields = Vec::with_capacity(headers.len());
    for (name, index) in preferred
        .map(|(name, index)| (name, *index))
        .chain(fallback)
    {
        if !seen.insert((name, index)) {
            continue;
        }
        if let Some(value) = headers.get_all(name).iter().nth(index) {
            let original = cases
                .and_then(|cases| cases.get_all(name).nth(index))
                .map_or_else(
                    || name.as_str().as_bytes().to_vec(),
                    |name| name.as_ref().to_vec(),
                );
            fields.push(HeaderField::try_new(original, value.as_bytes())?);
        }
    }
    Ok(HeaderBlock::from_fields(fields))
}
