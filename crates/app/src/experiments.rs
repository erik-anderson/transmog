//! Exact response collection selection and independent page association writes.
use super::{AutomationRegistry, AutomationRuleSet};
use crate::{AppError, ErrorCategory};
use serde::{Deserialize, Serialize};

/// An exact current collection or named version for frozen preview and export preparation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentSelection {
    /// Owning response set ID.
    pub experiment_id: String,
    /// Immutable owner incarnation; deletion/recreation cannot rebind a pending selection.
    pub incarnation: String,
    /// Empty selects current; otherwise the exact named version ID.
    pub variant_id: String,
    /// Exact current revision or immutable named version token.
    pub revision: u64,
}

pub(super) fn validate_owned_rules(
    current: &AutomationRuleSet,
    rules: &[transmog_automation::Rule],
) -> Result<(), AppError> {
    let owned = |rule: &&transmog_automation::Rule| {
        current.response_sets.iter().any(|set| set.owns(&rule.id))
    };
    if current.rules.iter().filter(owned).collect::<Vec<_>>()
        != rules.iter().filter(owned).collect::<Vec<_>>()
    {
        return Err(AppError::new(
            ErrorCategory::Conflict,
            "Response set rules changed outside their collection. Edit the response set or use its live controls.",
            true,
        ));
    }
    Ok(())
}

impl AutomationRegistry {
    pub(crate) fn set_experiment_preview_page(
        &self,
        id: &str,
        revision: u64,
        page: crate::ExperimentPreviewPage,
    ) -> Result<(), AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut document = current.document.clone();
        let set = document
            .response_sets
            .iter_mut()
            .find(|set| set.id == id)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::InvalidInput,
                    "Response set is unavailable.",
                    false,
                )
            })?;
        if set.preview_page_revision != revision {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "The page association changed. Review its current page before choosing again.",
                true,
            ));
        }
        set.preview_page = Some(page);
        set.preview_page_revision = revision.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCategory::InvalidInput,
                "Page association revision exhausted.",
                false,
            )
        })?;
        super::validate_document(&document)?;
        super::persistence_bytes(&document)?;
        if let Some(path) = self.path.as_deref() {
            super::persist(path, &document)?;
        }
        let mut replacement = (*current).clone();
        replacement.document = document;
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::sync::Arc::new(replacement);
        Ok(())
    }
}
