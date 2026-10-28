use super::{DesktopState, start_proxy_locked};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{Emitter as _, Manager as _, State};
use transmog_mcp::{
    AccessManager, AgentApplication, CallFuture, Error, GrantInput, LocalServer, ProxyHost, Scope,
};

struct DesktopProxy(DesktopState);
impl ProxyHost for DesktopProxy {
    fn start(&self) -> CallFuture<'_> {
        Box::pin(async move {
            let _operation = self.0.certificate_operation.lock().await;
            let request = transmog_app::ProxyStartRequest {
                ca_certificate_path: self.0.ca_certificate_path.clone(),
                ca_private_key_path: self.0.ca_private_key_path.clone(),
                ..transmog_app::ProxyStartRequest::default()
            };
            let status = start_proxy_locked(request, &self.0)
                .await
                .map_err(|message| Error {
                    code: "proxy_start_failed",
                    message,
                })?;
            serde_json::to_value(status).map_err(|_| Error {
                code: "internal",
                message: "Proxy status could not be encoded.".into(),
            })
        })
    }
}

pub(super) struct AgentState {
    root: PathBuf,
    access: Result<AccessManager, Error>,
    adapter: Result<Arc<AgentApplication>, Error>,
    listener: tokio::sync::Mutex<Option<LocalServer>>,
    maintenance: Mutex<Option<tokio::task::JoinHandle<()>>>,
    notifications: Mutex<Option<tokio::task::JoinHandle<()>>>,
    failure: Mutex<Option<String>>,
}

impl AgentState {
    pub fn new(state: DesktopState, root: PathBuf) -> Self {
        let access = AccessManager::open(Some(root.join("mcp-access.json")));
        let adapter = access.clone().and_then(|access| {
            AgentApplication::new(
                state.application.clone(),
                access,
                root.clone(),
                Arc::new(DesktopProxy(state)),
            )
        });
        Self {
            root,
            access,
            adapter,
            listener: tokio::sync::Mutex::new(None),
            maintenance: Mutex::new(None),
            notifications: Mutex::new(None),
            failure: Mutex::new(None),
        }
    }
    fn access(&self) -> Result<&AccessManager, Error> {
        self.access.as_ref().map_err(Clone::clone)
    }
    fn adapter(&self) -> Result<&Arc<AgentApplication>, Error> {
        self.adapter.as_ref().map_err(Clone::clone)
    }
    pub fn start(app: &tauri::AppHandle) {
        let handle = app.clone();
        let worker = tauri::async_runtime::spawn(async move {
            let state = handle.state::<Self>();
            if let Ok(adapter) = state.adapter() {
                let mut maintenance = state
                    .maintenance
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if maintenance.is_none() {
                    *maintenance = Some(adapter.start_maintenance());
                }
                let mut notifications = state
                    .notifications
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if notifications.is_none() {
                    let mut changes = adapter.subscribe_changes();
                    let app = handle.clone();
                    *notifications = Some(tokio::spawn(async move {
                        while changes.changed().await.is_ok() {
                            let _ = app.emit_to("main", "mcp-automation-changed", ());
                        }
                    }));
                }
            }
            if state.access().is_ok_and(|access| access.status().enabled)
                && let Err(error) = state.set_enabled(true).await
            {
                *state
                    .failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.message);
            }
        });
        // Startup owns its work through the application's runtime; it does not wait for UI hydration.
        drop(worker);
    }
    async fn set_enabled(&self, enabled: bool) -> Result<(), Error> {
        let mut listener = self.listener.lock().await;
        let access = self.access()?;
        let adapter = self.adapter()?;
        access.set_enabled(enabled)?;
        if !enabled {
            listener.take();
            adapter.end_all().await?;
        } else if listener.is_none() {
            *listener = Some(
                LocalServer::start_for_instance(
                    self.root.clone(),
                    adapter.instance_id().into(),
                    access.clone(),
                    adapter.clone(),
                )
                .await?,
            );
        }
        *self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }
    async fn status(&self) -> Result<Value, Error> {
        let access = self.access()?.status();
        let adapter = self.adapter()?;
        let listener = self.listener.lock().await;
        let command = std::env::current_exe()
            .ok()
            .map(|path| path.with_file_name("transmog-cli.exe"));
        let mut status = adapter.management_status();
        status["enabled"] = json!(access.enabled);
        status["grants"] = json!(access.grants);
        status["endpoint"] = json!(
            listener
                .as_ref()
                .map(|listener| format!("https://{}", listener.endpoint().address))
        );
        status["cliCommand"] = json!(command);
        // MCP clients may sanitize their subprocess environment. Carry the exact
        // desktop profile location explicitly so agent configuration can find it.
        status["cliEnvironment"] = json!({"LOCALAPPDATA":self.root.parent()});
        status["error"] = json!(
            self.failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        );
        status["temporaryRules"] = json!(adapter_rules(adapter));
        Ok(status)
    }
    fn validate_scope(&self, input: &GrantInput) -> Result<(), Error> {
        if let Scope::Requests { instance_id, .. } = &input.scope
            && instance_id != self.adapter()?.instance_id()
        {
            return Err(Error {
                code: "stale_instance",
                message: "Refresh the selected-request scope after restarting Transmog.".into(),
            });
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.listener.lock().await.take();
        if let Some(notifications) = self
            .notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            notifications.abort();
        }
        if let Some(maintenance) = self
            .maintenance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            maintenance.abort();
        }
        if let Ok(adapter) = self.adapter() {
            adapter.end_all().await?;
        }
        if let Ok(access) = self.access() {
            access.flush()?;
        }
        Ok(())
    }
}

fn adapter_rules(adapter: &AgentApplication) -> Value {
    adapter.temporary_rules()
}

#[tauri::command]
pub(super) fn mcp_summary(state: State<'_, AgentState>) -> Value {
    json!({"enabled":state.access().is_ok_and(|access|access.status().enabled),"controls":state.adapter().map_or(0,|adapter|adapter.active_control_count())})
}

#[tauri::command]
pub(super) async fn mcp_status(state: State<'_, AgentState>) -> Result<Value, Error> {
    state.status().await
}

#[tauri::command]
pub(super) async fn mcp_set_enabled(
    enabled: bool,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    if let Err(error) = state.set_enabled(enabled).await {
        *state
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.message.clone());
        return Err(error);
    }
    state.status().await
}

#[tauri::command]
pub(super) async fn mcp_create_key(
    input: GrantInput,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    state.validate_scope(&input)?;
    let (grant, key) = state.access()?.create(input)?;
    Ok(json!({"grant":grant,"key":key.expose(),"status":state.status().await?}))
}

#[tauri::command]
pub(super) async fn mcp_update_key(
    id: String,
    revision: u64,
    input: GrantInput,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    state.validate_scope(&input)?;
    state.access()?.update(&id, revision, input)?;
    state.adapter()?.reconcile().await?;
    state.status().await
}

#[tauri::command]
pub(super) async fn mcp_replace_key(
    id: String,
    revision: u64,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    let key = state.access()?.replace(&id, revision)?;
    // The replacement is already durable. Always deliver its one-time secret, even
    // when temporary-effect cleanup fails; maintenance will retry that cleanup.
    if let Err(error) = state.adapter()?.reconcile().await {
        *state
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.message);
    }
    Ok(json!({"key":key.expose(),"status":state.status().await?}))
}

#[tauri::command]
pub(super) async fn mcp_revoke_key(
    id: Option<String>,
    revision: Option<u64>,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    let target = match (id.as_deref(), revision) {
        (Some(id), Some(revision)) => Some((id, revision)),
        (None, None) => None,
        _ => {
            return Err(Error {
                code: "invalid_input",
                message: "Choose a current key revision.".into(),
            });
        }
    };
    state.access()?.revoke(target)?;
    state.adapter()?.reconcile().await?;
    state.status().await
}

#[tauri::command]
pub(super) async fn mcp_remove_key(
    id: String,
    revision: u64,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    state.access()?.remove(&id, revision)?;
    state.status().await
}

#[tauri::command]
pub(super) async fn mcp_end_control(
    id: String,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    state.adapter()?.end_from_desktop(&id).await?;
    state.status().await
}

#[tauri::command]
pub(super) async fn mcp_keep_rule(
    id: String,
    revision: u64,
    state: State<'_, AgentState>,
) -> Result<Value, Error> {
    state.adapter()?.keep_rule(&id, revision).await?;
    state.status().await
}
