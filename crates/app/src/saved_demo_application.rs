//! Owner-admitted desktop operations for immutable saved demos.
use crate::{
    AppError, Application, CapturedPage, ErrorCategory, SaveDemoInput, SavedDemo,
    SavedDemoPreparation, SavedDemoRevision,
};
use std::sync::{Arc, atomic::AtomicBool};

impl Application {
    /// Lists this response set's durable demos without consulting retained Traffic.
    /// # Errors
    /// Reports a deleted response set.
    pub fn saved_demos(&self, owner_id: &str) -> Result<Vec<SavedDemo>, AppError> {
        let _operation = self
            .response_set_operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let incarnation = self.demo_owner(owner_id)?;
        self.saved_demos.admit_owner(owner_id, &incarnation)?;
        Ok(self.saved_demos.list(owner_id, &incarnation))
    }

    /// Freezes one exact saved revision and an explicitly chosen captured page for review.
    /// The returned token commits these same bytes even if linked files later change.
    /// # Errors
    /// Reports stale selections, incomplete HTML, unavailable assets, limits or cancellation.
    pub async fn prepare_demo_save(
        &self,
        input: SaveDemoInput,
        canceled: Arc<AtomicBool>,
    ) -> Result<SavedDemoPreparation, AppError> {
        let incarnation = {
            let _operation = self
                .response_set_operation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let status = self.automation.status();
            let owner = status
                .response_sets
                .iter()
                .find(|set| set.id == input.owner_id)
                .ok_or_else(owner_unavailable)?;
            let selection = input
                .options
                .variant
                .as_ref()
                .ok_or_else(|| invalid("Choose the saved response set revision to include"))?;
            if input.options.rule_ids.is_some()
                || input.options.case_preview.is_some()
                || owner.selected_rules(selection).is_none()
            {
                return Err(invalid(
                    "The saved response selection changed; reopen Save demo",
                ));
            }
            self.saved_demos
                .admit_owner(&owner.id, &owner.incarnation)?;
            owner.incarnation.clone()
        };
        let page = self
            .prepare_captured_page_with_options(input.entry_id, input.options, canceled)
            .await?;
        let _operation = self
            .response_set_operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.demo_owner(&input.owner_id)? != incarnation {
            return Err(owner_unavailable());
        }
        self.saved_demos.stage(
            &input.owner_id,
            &incarnation,
            &input.name,
            input.replace,
            page,
        )
    }

    /// Saves the exact reviewed snapshot. Missing resources require explicit review.
    /// # Errors
    /// Reports expired tokens, stale replacements, deleted owners, cancellation or disk failures.
    pub async fn commit_demo_save(
        &self,
        token: String,
        reviewed_missing: bool,
        canceled: Arc<AtomicBool>,
    ) -> Result<SavedDemo, AppError> {
        let application = self.clone();
        tokio::task::spawn_blocking(move || {
            let _operation = application
                .response_set_operation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (owner, incarnation) = application.saved_demos.prepared_owner(&token)?;
            if application.demo_owner(&owner)? != incarnation {
                return Err(owner_unavailable());
            }
            application
                .saved_demos
                .commit(&token, reviewed_missing, &canceled)
        })
        .await
        .map_err(|_| invalid("Saving the demo could not finish; retry"))?
    }

    /// Releases an unsaved prepared snapshot without publishing a demo.
    pub fn cancel_demo_save(&self, token: &str) {
        self.saved_demos.cancel(token);
    }

    /// Deletes one exact reviewed demo revision without affecting other retained evidence.
    /// # Errors
    /// Reports stale revisions, deleted owners or storage failure before publication.
    pub fn delete_saved_demo(
        &self,
        owner_id: &str,
        expected: &SavedDemoRevision,
    ) -> Result<(), AppError> {
        let _operation = self
            .response_set_operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let incarnation = self.demo_owner(owner_id)?;
        self.saved_demos.admit_owner(owner_id, &incarnation)?;
        self.saved_demos.delete(owner_id, &incarnation, expected)
    }

    /// Opens an independent frozen run from durable bytes, with no current-rule lookup.
    /// The native preview must still obtain its own script choice and enforce isolation.
    /// # Errors
    /// Reports stale selections, deleted owners, cancellation or failed byte integrity.
    pub async fn prepare_saved_demo(
        &self,
        owner_id: String,
        expected: SavedDemoRevision,
        canceled: Arc<AtomicBool>,
    ) -> Result<CapturedPage, AppError> {
        let application = self.clone();
        tokio::task::spawn_blocking(move || {
            let _operation = application
                .response_set_operation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let incarnation = application.demo_owner(&owner_id)?;
            application
                .saved_demos
                .admit_owner(&owner_id, &incarnation)?;
            application
                .saved_demos
                .open(&owner_id, &incarnation, &expected, &canceled)
        })
        .await
        .map_err(|_| invalid("Opening the saved demo could not finish; retry"))?
    }

    fn demo_owner(&self, owner_id: &str) -> Result<String, AppError> {
        self.automation
            .status()
            .response_sets
            .iter()
            .find(|set| set.id == owner_id)
            .map(|set| set.incarnation.clone())
            .ok_or_else(owner_unavailable)
    }
}

fn owner_unavailable() -> AppError {
    AppError::new(
        ErrorCategory::Conflict,
        "This response set is unavailable; reopen Response sets",
        false,
    )
}
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
