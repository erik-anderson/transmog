//! Private inspection state with references to the application's single live engine.
use crate::{AppConfig, AppError, Application, BodyStoreConfig, ErrorCategory};
use std::{path::PathBuf, sync::Arc};
use transmog_core::intercept::ExchangeId;

impl Application {
    /// Loaded file-group identities, independent of visible rows and Undo.
    pub fn loaded_capture_ids(&self) -> Vec<String> {
        self.traces.group_ids()
    }
    /// Number of references to shared live evidence.
    pub fn live_membership_count(&self) -> usize {
        self.service.catalog().reference_count()
    }
    /// Current request-side admission metadata for one live exchange.
    pub fn live_exchange_membership(&self, id: &str) -> Option<(String, u64, String, String)> {
        let exchange = ExchangeId(crate::inspector::parse_session_id(id).ok()?);
        let (number, method) = self
            .service
            .catalog()
            .with_heads(exchange, |heads| {
                (!heads.imported).then(|| {
                    (
                        heads.request_number,
                        heads
                            .request_heads
                            .first()
                            .map_or_else(String::new, |head| head.head.method.clone()),
                    )
                })
            })
            .flatten()?;
        self.copy_message_url(id, "client-request")
            .ok()
            .map(|url| (id.into(), number, url, method))
    }
    /// Canonical live admissions, excluding imported files. Payloads are not copied.
    pub fn live_exchange_memberships(&self) -> Vec<(String, u64, String, String)> {
        let entries = self.service.catalog().project_retained(|entry| {
            (!entry.imported).then(|| {
                (
                    format!("{:032x}", entry.exchange_id.0),
                    entry.request_number,
                    entry
                        .request_heads
                        .first()
                        .map_or_else(String::new, |head| head.head.method.clone()),
                )
            })
        });
        entries
            .into_iter()
            .flatten()
            .filter_map(|(id, number, method)| {
                self.copy_message_url(&id, "client-request")
                    .ok()
                    .map(|url| (id, number, url, method))
            })
            .collect()
    }
    /// Creates an empty inspection backend without a window, listener or shared-rule changes.
    /// # Errors
    /// Returns cache or bounded service initialization failures.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "The public factory accepts an owned session-storage location."
    )]
    pub fn create_inspection_session(&self, root: PathBuf) -> Result<Self, AppError> {
        let mut config = BodyStoreConfig::product_default(root.join("bodies"));
        config.use_product_preferences = false;
        let mut session = Self::new(AppConfig {
            body_store: Some(config),
            extensions: Some(self.extensions()),
            ..AppConfig::default()
        })?;
        session.composer = self.composer.fork();
        session.traces.retain_group_sources();
        session.save_product_state(self.product_state())?;
        session.previews = self.previews.clone();
        if let (Some(store), Some(source)) = (session.body_store.take(), self.body_store.clone()) {
            session.body_store =
                Some(store.with_live_source(source, session.service.catalog().clone()));
        }
        session.live_parent = Some(Arc::new(self.clone()));
        Ok(session)
    }
    /// Adds memberships to canonical live exchanges without replay or duplicated body storage.
    /// # Errors
    /// Returns invalid IDs, a missing live source, or a finite membership-limit error.
    pub fn reference_live_exchanges(&self, ids: &[String]) -> Result<usize, AppError> {
        let source = self.live_parent.as_ref().ok_or_else(|| {
            AppError::new(
                ErrorCategory::InvalidInput,
                "This is the live workspace",
                false,
            )
        })?;
        let ids = ids
            .iter()
            .map(|id| crate::inspector::parse_session_id(id).map(ExchangeId))
            .collect::<Result<Vec<_>, _>>()?;
        let store = source.body_store.clone();
        self.service
            .catalog()
            .reference_exchanges(
                source.service.catalog(),
                &ids,
                Arc::new(move |released| {
                    if let Some(store) = &store {
                        let _ = store.forget_entries(&released);
                    }
                }),
            )
            .map_err(|error| AppError::new(ErrorCategory::Limit, error.to_string(), false))
    }
    /// Sends a presentation refresh for a referenced canonical exchange.
    /// # Errors
    /// Returns an invalid exchange identity.
    pub fn refresh_live_exchange(&self, id: &str) -> Result<(), AppError> {
        self.service
            .catalog()
            .refresh_references(ExchangeId(crate::inspector::parse_session_id(id)?));
        Ok(())
    }
    /// Number of owned imported-source index bytes, excluding shared live evidence.
    pub fn imported_index_bytes(&self) -> u64 {
        self.traces.retained_index_bytes()
    }
    /// Unloads one complete imported file group without deleting its source file.
    /// # Errors
    /// Rejects missing captures or unsaved human marker changes; reports cache cleanup errors.
    pub fn unload_capture(&self, id: &str) -> Result<(), AppError> {
        let _publication = self.traces.lock_publication();
        if self.traces.has_marker_changes() {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "Resolve unsaved marker changes before unloading this capture",
                false,
            ));
        }
        let ids = self.traces.group_entries(id)?;
        self.service.catalog().dismiss(&ids, &[]);
        let removed = self.service.catalog().forget_dismissed(&ids);
        if let Some(store) = &self.body_store {
            store.forget_entries(&removed).map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "Capture readers could not be released",
                    true,
                )
            })?;
        }
        self.traces.forget_group(id, &removed);
        Ok(())
    }
    /// Whether the review backend has unsaved marker edits.
    pub fn has_unsaved_session_changes(&self) -> bool {
        self.traces.has_marker_changes()
    }
}
