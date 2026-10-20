//! Validate context continuation scope, ordering, declarations and lifetime totals.
use super::{Pending, fail, project, protocol};
use std::collections::BTreeMap;
use transmog_extensions::{
    ContextPage, ContextPosition, ContextStatus, Failure, FailureKind, Visualizer, decimal,
    valid_id,
};

pub(super) fn resume(
    token: &str,
    page: &mut ContextPage,
    visualizer: &Visualizer,
    pending: &mut Pending,
    grants: &mut BTreeMap<String, u64>,
    counters: (&mut usize, &mut usize),
) -> Result<(), Failure> {
    let (remaining, continuations) = counters;
    let request = pending.context.as_ref().ok_or_else(protocol)?;
    let context = visualizer.context.as_ref().ok_or_else(protocol)?;
    if pending.token.as_deref() != Some(token)
        || page.exchanges.len() > request.limit
        || decimal(&page.anchor_order).is_err()
        || page
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| !valid_id(cursor))
        || page.next_cursor.is_some() != (page.status == ContextStatus::More)
    {
        return Err(protocol());
    }
    *continuations += 1;
    pending.context_total = pending
        .context_total
        .checked_add(page.exchanges.len())
        .ok_or_else(protocol)?;
    if *continuations > 512 || pending.context_total > context.max_exchanges {
        return Err(fail(FailureKind::Limit, "Related context budget exhausted"));
    }
    let anchor = decimal(&page.anchor_order).map_err(|_| protocol())?;
    let mut prior = None;
    for exchange in &mut page.exchanges {
        let order = decimal(&exchange.order).map_err(|_| protocol())?;
        if prior.is_some_and(|prior| prior >= order)
            || match request.position {
                ContextPosition::Before => order >= anchor,
                ContextPosition::After => order <= anchor,
            }
        {
            return Err(protocol());
        }
        prior = Some(order);
        project(
            &mut exchange.input,
            visualizer,
            grants,
            &mut pending.decodable,
            remaining,
        )?;
    }
    Ok(())
}
