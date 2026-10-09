use crate::{AppError, Application};
use serde::Serialize;
use std::time::Duration;
use transmog_core::intercept::ExchangeId;

/// Result of clearing every retained traffic entry, independent of filters.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficClearResult {
    /// Removed entry identifiers.
    pub ids: Vec<String>,
    /// Retained body and header bytes associated with the removed entries.
    pub bytes: u64,
    /// Whether the removed data can be restored.
    pub undoable: bool,
    /// Undo lifetime in seconds, when data is at least 100 MB.
    pub undo_seconds: Option<u64>,
}
fn policy(bytes: u64) -> (bool, Option<u64>) {
    if bytes >= 1_000_000_000 {
        (false, None)
    } else if bytes >= 100_000_000 {
        (true, Some(300))
    } else {
        (true, None)
    }
}
pub(crate) fn clear(application: &Application) -> Result<TrafficClearResult, AppError> {
    if let Some(store) = &application.body_store {
        store.flush().map_err(|_| AppError::new(crate::ErrorCategory::Unavailable, "Traffic cache could not be synchronized for Clear", true))?;
    }
    let ids = application.service.catalog().dismiss_all();
    let bytes = ids.iter().fold(
        application.service.catalog().retained_header_bytes(&ids),
        |sum, id| {
            sum.saturating_add(application.body_store.as_ref().map_or(0, |store| {
                store
                    .metadata(*id)
                    .iter()
                    .map(|body| body.retained_bytes)
                    .fold(0_u64, u64::saturating_add)
            }))
        },
    );
    let (undoable, undo_seconds) = policy(bytes);
    if !undoable {
        release(application, &ids)?;
    } else if let Some(seconds) = undo_seconds {
        let service = application.service.clone();
        let store = application.body_store.clone();
        let traces = application.traces.clone();
        let expired = service.catalog().dismissal_versions(&ids);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let removed = service.catalog().forget_dismissed_versions(&expired);
            if let Some(store) = store {
                let _ = store.forget_entries(&removed);
            }
            traces.forget_entries(&removed);
        });
    }
    Ok(TrafficClearResult {
        ids: ids.iter().map(|id| format!("{:032x}", id.0)).collect(),
        bytes,
        undoable,
        undo_seconds,
    })
}
fn release(application: &Application, ids: &[ExchangeId]) -> Result<(), AppError> {
    let removed = application.service.catalog().forget_dismissed(ids);
    if let Some(store) = &application.body_store {
        store.forget_entries(&removed).map_err(|_| {
            AppError::new(
                crate::ErrorCategory::Unavailable,
                "Cleared body data could not be released",
                true,
            )
        })?;
    }
    application.traces.forget_entries(&removed);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_undo_thresholds_include_exact_boundaries() {
        assert_eq!(policy(99_999_999), (true, None));
        assert_eq!(policy(100_000_000), (true, Some(300)));
        assert_eq!(policy(999_999_999), (true, Some(300)));
        assert_eq!(policy(1_000_000_000), (false, None));
    }
}
