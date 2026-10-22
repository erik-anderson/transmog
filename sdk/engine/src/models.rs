// Generated from the canonical Rust models. Do not edit.
use super::abi::*;
use transmog_app::AppError;
pub fn app_lifecycle_number(value: &transmog_app::AppLifecycle) -> u32 {
    match value {
        transmog_app::AppLifecycle::Stopped => 0,
        transmog_app::AppLifecycle::Running => 1,
        transmog_app::AppLifecycle::Draining => 2,
        transmog_app::AppLifecycle::Stopping => 3,
        transmog_app::AppLifecycle::Failed => 4,
    }
}
impl Project for transmog_app::AppLifecycle {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(app_lifecycle_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewAppStatus {
    pub lifecycle: u32,
    pub listener: Optional<Text>,
    pub summary: Text,
    pub host_restore_pending: u32,
}
impl Project for transmog_app::AppStatus {
    type View = ViewAppStatus;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewAppStatus {
            lifecycle: self.lifecycle.project(arena)?,
            listener: self.listener.project(arena)?,
            summary: self.summary.project(arena)?,
            host_restore_pending: self.host_restore_pending.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewAllowedHttpProtocols {
    pub http1: u32,
    pub http2: u32,
    pub http3: u32,
}
impl Project for transmog_app::AllowedHttpProtocols {
    type View = ViewAllowedHttpProtocols;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewAllowedHttpProtocols {
            http1: self.http1.project(arena)?,
            http2: self.http2.project(arena)?,
            http3: self.http3.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::AllowedHttpProtocols {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            http1: unsafe { FromAbi::read(&value.http1, budget) }?,
            http2: unsafe { FromAbi::read(&value.http2, budget) }?,
            http3: unsafe { FromAbi::read(&value.http3, budget) }?,
        })
    }
}
pub fn tls_versions_number(value: &transmog_app::TlsVersions) -> u32 {
    match value {
        transmog_app::TlsVersions::Tls12And13 => 0,
        transmog_app::TlsVersions::Tls12Only => 1,
        transmog_app::TlsVersions::Tls13Only => 2,
    }
}
impl Project for transmog_app::TlsVersions {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(tls_versions_number(self))
    }
}
impl FromAbi for transmog_app::TlsVersions {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Tls12And13,
            1 => Self::Tls12Only,
            2 => Self::Tls13Only,
            _ => return Err(invalid("Invalid TlsVersions discriminant")),
        })
    }
}
pub fn tls_grease_number(value: &transmog_app::TlsGrease) -> u32 {
    match value {
        transmog_app::TlsGrease::Chromium => 0,
        transmog_app::TlsGrease::NoSignature => 1,
        transmog_app::TlsGrease::Disabled => 2,
    }
}
impl Project for transmog_app::TlsGrease {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(tls_grease_number(self))
    }
}
impl FromAbi for transmog_app::TlsGrease {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Chromium,
            1 => Self::NoSignature,
            2 => Self::Disabled,
            _ => return Err(invalid("Invalid TlsGrease discriminant")),
        })
    }
}
pub fn root_store_number(value: &transmog_app::RootStore) -> u32 {
    match value {
        transmog_app::RootStore::Os => 0,
        transmog_app::RootStore::ChromiumBundled => 1,
        transmog_app::RootStore::ChromeInstalled => 2,
        transmog_app::RootStore::Webview2Installed => 3,
    }
}
impl Project for transmog_app::RootStore {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(root_store_number(self))
    }
}
impl FromAbi for transmog_app::RootStore {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Os,
            1 => Self::ChromiumBundled,
            2 => Self::ChromeInstalled,
            3 => Self::Webview2Installed,
            _ => return Err(invalid("Invalid RootStore discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewNetworkProfileSettings {
    pub protocols: ViewAllowedHttpProtocols,
    pub reuse_connections: u32,
    pub tls_versions: u32,
    pub compatibility_ciphers: u32,
    pub hybrid_key_exchange: u32,
    pub grease: u32,
    pub randomize_extensions: u32,
    pub ech_grease: u32,
    pub encrypted_client_hello: u32,
    pub certificate_compression: u32,
    pub http2_alps: u32,
    pub http3_alps: u32,
    pub trust_anchor_ids: u32,
    pub pki_metadata_file: Optional<Text>,
    pub root_store: u32,
    pub local_certificates: u32,
}
impl Project for transmog_app::NetworkProfileSettings {
    type View = ViewNetworkProfileSettings;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewNetworkProfileSettings {
            protocols: self.protocols.project(arena)?,
            reuse_connections: self.reuse_connections.project(arena)?,
            tls_versions: self.tls_versions.project(arena)?,
            compatibility_ciphers: self.compatibility_ciphers.project(arena)?,
            hybrid_key_exchange: self.hybrid_key_exchange.project(arena)?,
            grease: self.grease.project(arena)?,
            randomize_extensions: self.randomize_extensions.project(arena)?,
            ech_grease: self.ech_grease.project(arena)?,
            encrypted_client_hello: self.encrypted_client_hello.project(arena)?,
            certificate_compression: self.certificate_compression.project(arena)?,
            http2_alps: self.http2_alps.project(arena)?,
            http3_alps: self.http3_alps.project(arena)?,
            trust_anchor_ids: self.trust_anchor_ids.project(arena)?,
            pki_metadata_file: self.pki_metadata_file.project(arena)?,
            root_store: self.root_store.project(arena)?,
            local_certificates: self.local_certificates.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::NetworkProfileSettings {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            protocols: unsafe { FromAbi::read(&value.protocols, budget) }?,
            reuse_connections: unsafe { FromAbi::read(&value.reuse_connections, budget) }?,
            tls_versions: unsafe { FromAbi::read(&value.tls_versions, budget) }?,
            compatibility_ciphers: unsafe { FromAbi::read(&value.compatibility_ciphers, budget) }?,
            hybrid_key_exchange: unsafe { FromAbi::read(&value.hybrid_key_exchange, budget) }?,
            grease: unsafe { FromAbi::read(&value.grease, budget) }?,
            randomize_extensions: unsafe { FromAbi::read(&value.randomize_extensions, budget) }?,
            ech_grease: unsafe { FromAbi::read(&value.ech_grease, budget) }?,
            encrypted_client_hello: unsafe {
                FromAbi::read(&value.encrypted_client_hello, budget)
            }?,
            certificate_compression: unsafe {
                FromAbi::read(&value.certificate_compression, budget)
            }?,
            http2_alps: unsafe { FromAbi::read(&value.http2_alps, budget) }?,
            http3_alps: unsafe { FromAbi::read(&value.http3_alps, budget) }?,
            trust_anchor_ids: unsafe { FromAbi::read(&value.trust_anchor_ids, budget) }?,
            pki_metadata_file: unsafe { FromAbi::read(&value.pki_metadata_file, budget) }?,
            root_store: unsafe { FromAbi::read(&value.root_store, budget) }?,
            local_certificates: unsafe { FromAbi::read(&value.local_certificates, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewNetworkProfileSnapshot {
    pub id: Text,
    pub name: Text,
    pub revision: u64,
    pub base_profile: Text,
    pub settings: ViewNetworkProfileSettings,
}
impl Project for transmog_app::NetworkProfileSnapshot {
    type View = ViewNetworkProfileSnapshot;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewNetworkProfileSnapshot {
            id: self.id.project(arena)?,
            name: self.name.project(arena)?,
            revision: self.revision.project(arena)?,
            base_profile: self.base_profile.project(arena)?,
            settings: self.settings.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::NetworkProfileSnapshot {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            id: unsafe { FromAbi::read(&value.id, budget) }?,
            name: unsafe { FromAbi::read(&value.name, budget) }?,
            revision: unsafe { FromAbi::read(&value.revision, budget) }?,
            base_profile: unsafe { FromAbi::read(&value.base_profile, budget) }?,
            settings: unsafe { FromAbi::read(&value.settings, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewNetworkProfiles {
    pub revision: u64,
    pub active_profile: ViewNetworkProfileSnapshot,
    pub custom_profiles: Slice<ViewNetworkProfileSnapshot>,
}
impl Project for transmog_app::NetworkProfiles {
    type View = ViewNetworkProfiles;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewNetworkProfiles {
            revision: self.revision.project(arena)?,
            active_profile: self.active_profile.project(arena)?,
            custom_profiles: self.custom_profiles.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::NetworkProfiles {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            revision: unsafe { FromAbi::read(&value.revision, budget) }?,
            active_profile: unsafe { FromAbi::read(&value.active_profile, budget) }?,
            custom_profiles: unsafe { FromAbi::read(&value.custom_profiles, budget) }?,
        })
    }
}
pub fn client_certificate_default_number(
    value: &transmog_core::client_authentication::ClientCertificateDefault,
) -> u32 {
    match value {
        transmog_core::client_authentication::ClientCertificateDefault::NoCertificate => 0,
        transmog_core::client_authentication::ClientCertificateDefault::Prompt => 1,
    }
}
impl Project for transmog_core::client_authentication::ClientCertificateDefault {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(client_certificate_default_number(self))
    }
}
impl FromAbi for transmog_core::client_authentication::ClientCertificateDefault {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::NoCertificate,
            1 => Self::Prompt,
            _ => return Err(invalid("Invalid ClientCertificateDefault discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClientCertificateSelector {
    pub identity: Optional<Text>,
    pub subject: Slice<TextEntry>,
    pub issuer: Slice<TextEntry>,
}
impl Project for transmog_core::client_authentication::ClientCertificateSelector {
    type View = ViewClientCertificateSelector;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewClientCertificateSelector {
            identity: self.identity.project(arena)?,
            subject: self.subject.project(arena)?,
            issuer: self.issuer.project(arena)?,
        })
    }
}
impl FromAbi for transmog_core::client_authentication::ClientCertificateSelector {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            identity: unsafe { FromAbi::read(&value.identity, budget) }?,
            subject: unsafe { FromAbi::read(&value.subject, budget) }?,
            issuer: unsafe { FromAbi::read(&value.issuer, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClientCertificateActionAutomatic {
    pub selector: ViewClientCertificateSelector,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClientCertificateAction {
    pub kind: u32,
    pub automatic: ViewClientCertificateActionAutomatic,
}
impl Project for transmog_core::client_authentication::ClientCertificateAction {
    type View = ViewClientCertificateAction;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(match self {
            Self::NoCertificate => ViewClientCertificateAction {
                kind: 0,
                ..Default::default()
            },
            Self::Automatic { selector } => ViewClientCertificateAction {
                kind: 1,
                automatic: ViewClientCertificateActionAutomatic {
                    selector: selector.project(arena)?,
                },
            },
            Self::Prompt => ViewClientCertificateAction {
                kind: 2,
                ..Default::default()
            },
        })
    }
}
impl FromAbi for transmog_core::client_authentication::ClientCertificateAction {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match value.kind {
            0 => Self::NoCertificate,
            1 => Self::Automatic {
                selector: unsafe { FromAbi::read(&value.automatic.selector, budget) }?,
            },
            2 => Self::Prompt,
            _ => return Err(invalid("Invalid ClientCertificateAction discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClientCertificateRule {
    pub id: Text,
    pub enabled: u32,
    pub host: Text,
    pub port: u16,
    pub mode: ViewClientCertificateAction,
}
impl Project for transmog_core::client_authentication::ClientCertificateRule {
    type View = ViewClientCertificateRule;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewClientCertificateRule {
            id: self.id.project(arena)?,
            enabled: self.enabled.project(arena)?,
            host: self.host.project(arena)?,
            port: self.port.project(arena)?,
            mode: self.mode.project(arena)?,
        })
    }
}
impl FromAbi for transmog_core::client_authentication::ClientCertificateRule {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            id: unsafe { FromAbi::read(&value.id, budget) }?,
            enabled: unsafe { FromAbi::read(&value.enabled, budget) }?,
            host: unsafe { FromAbi::read(&value.host, budget) }?,
            port: unsafe { FromAbi::read(&value.port, budget) }?,
            mode: unsafe { FromAbi::read(&value.mode, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClientCertificatePolicy {
    pub default: u32,
    pub rules: Slice<ViewClientCertificateRule>,
}
impl Project for transmog_app::ClientCertificatePolicy {
    type View = ViewClientCertificatePolicy;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewClientCertificatePolicy {
            default: self.default.project(arena)?,
            rules: self.rules.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::ClientCertificatePolicy {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            default: unsafe { FromAbi::read(&value.default, budget) }?,
            rules: unsafe { FromAbi::read(&value.rules, budget) }?,
        })
    }
}
pub fn theme_preference_number(value: &transmog_app::ThemePreference) -> u32 {
    match value {
        transmog_app::ThemePreference::System => 0,
        transmog_app::ThemePreference::Light => 1,
        transmog_app::ThemePreference::Dark => 2,
    }
}
impl Project for transmog_app::ThemePreference {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(theme_preference_number(self))
    }
}
impl FromAbi for transmog_app::ThemePreference {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::System,
            1 => Self::Light,
            2 => Self::Dark,
            _ => return Err(invalid("Invalid ThemePreference discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewProductPreferences {
    pub network_profiles: ViewNetworkProfiles,
    pub allow_remote_clients: u32,
    pub proxy_port: Optional<u16>,
    pub reuse_server_connections: u32,
    pub client_certificate_policy: ViewClientCertificatePolicy,
    pub decrypt_https: u32,
    pub isolate_outbound_connections: u32,
    pub allowed_outbound_protocols: ViewAllowedHttpProtocols,
    pub theme: u32,
    pub session_page_size: usize,
    pub configure_system_proxy: u32,
}
impl Project for transmog_app::ProductPreferences {
    type View = ViewProductPreferences;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewProductPreferences {
            network_profiles: self.network_profiles.project(arena)?,
            allow_remote_clients: self.allow_remote_clients.project(arena)?,
            proxy_port: self.proxy_port.project(arena)?,
            reuse_server_connections: self.reuse_server_connections.project(arena)?,
            client_certificate_policy: self.client_certificate_policy.project(arena)?,
            decrypt_https: self.decrypt_https.project(arena)?,
            isolate_outbound_connections: self.isolate_outbound_connections.project(arena)?,
            allowed_outbound_protocols: self.allowed_outbound_protocols.project(arena)?,
            theme: self.theme.project(arena)?,
            session_page_size: self.session_page_size.project(arena)?,
            configure_system_proxy: self.configure_system_proxy.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::ProductPreferences {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            network_profiles: unsafe { FromAbi::read(&value.network_profiles, budget) }?,
            allow_remote_clients: unsafe { FromAbi::read(&value.allow_remote_clients, budget) }?,
            proxy_port: unsafe { FromAbi::read(&value.proxy_port, budget) }?,
            reuse_server_connections: unsafe {
                FromAbi::read(&value.reuse_server_connections, budget)
            }?,
            client_certificate_policy: unsafe {
                FromAbi::read(&value.client_certificate_policy, budget)
            }?,
            decrypt_https: unsafe { FromAbi::read(&value.decrypt_https, budget) }?,
            isolate_outbound_connections: unsafe {
                FromAbi::read(&value.isolate_outbound_connections, budget)
            }?,
            allowed_outbound_protocols: unsafe {
                FromAbi::read(&value.allowed_outbound_protocols, budget)
            }?,
            theme: unsafe { FromAbi::read(&value.theme, budget) }?,
            session_page_size: unsafe { FromAbi::read(&value.session_page_size, budget) }?,
            configure_system_proxy: unsafe {
                FromAbi::read(&value.configure_system_proxy, budget)
            }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewBufferLimitCustom {
    pub bytes: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewBufferLimit {
    pub kind: u32,
    pub custom: ViewBufferLimitCustom,
}
impl Project for transmog_app::BufferLimit {
    type View = ViewBufferLimit;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(match self {
            Self::Automatic => ViewBufferLimit {
                kind: 0,
                ..Default::default()
            },
            Self::Custom { bytes } => ViewBufferLimit {
                kind: 1,
                custom: ViewBufferLimitCustom {
                    bytes: bytes.project(arena)?,
                },
            },
            Self::Unlimited => ViewBufferLimit {
                kind: 2,
                ..Default::default()
            },
        })
    }
}
impl FromAbi for transmog_app::BufferLimit {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match value.kind {
            0 => Self::Automatic,
            1 => Self::Custom {
                bytes: unsafe { FromAbi::read(&value.custom.bytes, budget) }?,
            },
            2 => Self::Unlimited,
            _ => return Err(invalid("Invalid BufferLimit discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewPrivacySettings {
    pub terminate_websockets_on_clear: u32,
    pub buffer_limit: ViewBufferLimit,
    pub max_live_entries: Optional<usize>,
    pub retain_request_bodies: u32,
    pub request_body_limit: Optional<u64>,
    pub custom_request_body_limit: u32,
    pub redact_sensitive_headers: u32,
    pub retain_response_bodies: u32,
    pub retain_body_samples: u32,
    pub remember_recent_artifacts: u32,
    pub include_paths_in_support_bundles: u32,
}
impl Project for transmog_app::PrivacySettings {
    type View = ViewPrivacySettings;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewPrivacySettings {
            terminate_websockets_on_clear: self.terminate_websockets_on_clear.project(arena)?,
            buffer_limit: self.buffer_limit.project(arena)?,
            max_live_entries: self.max_live_entries.project(arena)?,
            retain_request_bodies: self.retain_request_bodies.project(arena)?,
            request_body_limit: self.request_body_limit.project(arena)?,
            custom_request_body_limit: self.custom_request_body_limit.project(arena)?,
            redact_sensitive_headers: self.redact_sensitive_headers.project(arena)?,
            retain_response_bodies: self.retain_response_bodies.project(arena)?,
            retain_body_samples: self.retain_body_samples.project(arena)?,
            remember_recent_artifacts: self.remember_recent_artifacts.project(arena)?,
            include_paths_in_support_bundles: self
                .include_paths_in_support_bundles
                .project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::PrivacySettings {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            terminate_websockets_on_clear: unsafe {
                FromAbi::read(&value.terminate_websockets_on_clear, budget)
            }?,
            buffer_limit: unsafe { FromAbi::read(&value.buffer_limit, budget) }?,
            max_live_entries: unsafe { FromAbi::read(&value.max_live_entries, budget) }?,
            retain_request_bodies: unsafe { FromAbi::read(&value.retain_request_bodies, budget) }?,
            request_body_limit: unsafe { FromAbi::read(&value.request_body_limit, budget) }?,
            custom_request_body_limit: unsafe {
                FromAbi::read(&value.custom_request_body_limit, budget)
            }?,
            redact_sensitive_headers: unsafe {
                FromAbi::read(&value.redact_sensitive_headers, budget)
            }?,
            retain_response_bodies: unsafe {
                FromAbi::read(&value.retain_response_bodies, budget)
            }?,
            retain_body_samples: unsafe { FromAbi::read(&value.retain_body_samples, budget) }?,
            remember_recent_artifacts: unsafe {
                FromAbi::read(&value.remember_recent_artifacts, budget)
            }?,
            include_paths_in_support_bundles: unsafe {
                FromAbi::read(&value.include_paths_in_support_bundles, budget)
            }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewWindowState {
    pub width: u32,
    pub height: u32,
    pub x: Optional<i32>,
    pub y: Optional<i32>,
    pub maximized: u32,
}
impl Project for transmog_app::WindowState {
    type View = ViewWindowState;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewWindowState {
            width: self.width.project(arena)?,
            height: self.height.project(arena)?,
            x: self.x.project(arena)?,
            y: self.y.project(arena)?,
            maximized: self.maximized.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::WindowState {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            width: unsafe { FromAbi::read(&value.width, budget) }?,
            height: unsafe { FromAbi::read(&value.height, budget) }?,
            x: unsafe { FromAbi::read(&value.x, budget) }?,
            y: unsafe { FromAbi::read(&value.y, budget) }?,
            maximized: unsafe { FromAbi::read(&value.maximized, budget) }?,
        })
    }
}
pub fn artifact_kind_number(value: &transmog_app::ArtifactKind) -> u32 {
    match value {
        transmog_app::ArtifactKind::NativeCapture => 0,
        transmog_app::ArtifactKind::JsonLines => 1,
        transmog_app::ArtifactKind::Saz => 2,
        transmog_app::ArtifactKind::Har => 3,
        transmog_app::ArtifactKind::Certificate => 4,
    }
}
impl Project for transmog_app::ArtifactKind {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(artifact_kind_number(self))
    }
}
impl FromAbi for transmog_app::ArtifactKind {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::NativeCapture,
            1 => Self::JsonLines,
            2 => Self::Saz,
            3 => Self::Har,
            4 => Self::Certificate,
            _ => return Err(invalid("Invalid ArtifactKind discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewRecentArtifact {
    pub path: Text,
    pub kind: u32,
}
impl Project for transmog_app::RecentArtifact {
    type View = ViewRecentArtifact;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewRecentArtifact {
            path: self.path.project(arena)?,
            kind: self.kind.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::RecentArtifact {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            path: unsafe { FromAbi::read(&value.path, budget) }?,
            kind: unsafe { FromAbi::read(&value.kind, budget) }?,
        })
    }
}
pub fn response_set_guide_history_number(value: &transmog_app::ResponseSetGuideHistory) -> u32 {
    match value {
        transmog_app::ResponseSetGuideHistory::NotOffered => 0,
        transmog_app::ResponseSetGuideHistory::Offered => 1,
        transmog_app::ResponseSetGuideHistory::Started => 2,
    }
}
impl Project for transmog_app::ResponseSetGuideHistory {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(response_set_guide_history_number(self))
    }
}
impl FromAbi for transmog_app::ResponseSetGuideHistory {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::NotOffered,
            1 => Self::Offered,
            2 => Self::Started,
            _ => return Err(invalid("Invalid ResponseSetGuideHistory discriminant")),
        })
    }
}
pub fn traffic_layout_number(value: &transmog_app::TrafficLayout) -> u32 {
    match value {
        transmog_app::TrafficLayout::Stacked => 0,
        transmog_app::TrafficLayout::SideBySide => 1,
    }
}
impl Project for transmog_app::TrafficLayout {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(traffic_layout_number(self))
    }
}
impl FromAbi for transmog_app::TrafficLayout {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Stacked,
            1 => Self::SideBySide,
            _ => return Err(invalid("Invalid TrafficLayout discriminant")),
        })
    }
}
pub fn traffic_column_number(value: &transmog_app::TrafficColumn) -> u32 {
    match value {
        transmog_app::TrafficColumn::RequestNumber => 0,
        transmog_app::TrafficColumn::Method => 1,
        transmog_app::TrafficColumn::Status => 2,
        transmog_app::TrafficColumn::Process => 3,
        transmog_app::TrafficColumn::Pid => 4,
        transmog_app::TrafficColumn::Host => 5,
        transmog_app::TrafficColumn::Path => 6,
        transmog_app::TrafficColumn::Url => 7,
        transmog_app::TrafficColumn::Protocol => 8,
        transmog_app::TrafficColumn::Duration => 9,
        transmog_app::TrafficColumn::HttpDuration => 10,
        transmog_app::TrafficColumn::StreamSent => 11,
        transmog_app::TrafficColumn::StreamReceived => 12,
        transmog_app::TrafficColumn::ResponseBytes => 13,
        transmog_app::TrafficColumn::RequestBytes => 14,
        transmog_app::TrafficColumn::State => 15,
        transmog_app::TrafficColumn::ContentType => 16,
        transmog_app::TrafficColumn::StartedAt => 17,
    }
}
impl Project for transmog_app::TrafficColumn {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(traffic_column_number(self))
    }
}
impl FromAbi for transmog_app::TrafficColumn {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::RequestNumber,
            1 => Self::Method,
            2 => Self::Status,
            3 => Self::Process,
            4 => Self::Pid,
            5 => Self::Host,
            6 => Self::Path,
            7 => Self::Url,
            8 => Self::Protocol,
            9 => Self::Duration,
            10 => Self::HttpDuration,
            11 => Self::StreamSent,
            12 => Self::StreamReceived,
            13 => Self::ResponseBytes,
            14 => Self::RequestBytes,
            15 => Self::State,
            16 => Self::ContentType,
            17 => Self::StartedAt,
            _ => return Err(invalid("Invalid TrafficColumn discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewColumnPreference {
    pub id: u32,
    pub width: u16,
    pub visible: u32,
    pub pinned: u32,
}
impl Project for transmog_app::ColumnPreference {
    type View = ViewColumnPreference;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewColumnPreference {
            id: self.id.project(arena)?,
            width: self.width.project(arena)?,
            visible: self.visible.project(arena)?,
            pinned: self.pinned.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::ColumnPreference {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            id: unsafe { FromAbi::read(&value.id, budget) }?,
            width: unsafe { FromAbi::read(&value.width, budget) }?,
            visible: unsafe { FromAbi::read(&value.visible, budget) }?,
            pinned: unsafe { FromAbi::read(&value.pinned, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewWorkspacePreferences {
    pub response_set_guide: u32,
    pub sidebar_collapsed: u32,
    pub layout: u32,
    pub list_split: u8,
    pub request_split: u8,
    pub request_body_split: u8,
    pub response_body_split: u8,
    pub autoresponse_split: u8,
    pub wrap_cells: u32,
    pub compact_rows: u32,
    pub columns: Slice<ViewColumnPreference>,
    pub extension_columns: Slice<Text>,
}
impl Project for transmog_app::WorkspacePreferences {
    type View = ViewWorkspacePreferences;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewWorkspacePreferences {
            response_set_guide: self.response_set_guide.project(arena)?,
            sidebar_collapsed: self.sidebar_collapsed.project(arena)?,
            layout: self.layout.project(arena)?,
            list_split: self.list_split.project(arena)?,
            request_split: self.request_split.project(arena)?,
            request_body_split: self.request_body_split.project(arena)?,
            response_body_split: self.response_body_split.project(arena)?,
            autoresponse_split: self.autoresponse_split.project(arena)?,
            wrap_cells: self.wrap_cells.project(arena)?,
            compact_rows: self.compact_rows.project(arena)?,
            columns: self.columns.project(arena)?,
            extension_columns: self.extension_columns.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::WorkspacePreferences {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            response_set_guide: unsafe { FromAbi::read(&value.response_set_guide, budget) }?,
            sidebar_collapsed: unsafe { FromAbi::read(&value.sidebar_collapsed, budget) }?,
            layout: unsafe { FromAbi::read(&value.layout, budget) }?,
            list_split: unsafe { FromAbi::read(&value.list_split, budget) }?,
            request_split: unsafe { FromAbi::read(&value.request_split, budget) }?,
            request_body_split: unsafe { FromAbi::read(&value.request_body_split, budget) }?,
            response_body_split: unsafe { FromAbi::read(&value.response_body_split, budget) }?,
            autoresponse_split: unsafe { FromAbi::read(&value.autoresponse_split, budget) }?,
            wrap_cells: unsafe { FromAbi::read(&value.wrap_cells, budget) }?,
            compact_rows: unsafe { FromAbi::read(&value.compact_rows, budget) }?,
            columns: unsafe { FromAbi::read(&value.columns, budget) }?,
            extension_columns: unsafe { FromAbi::read(&value.extension_columns, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewProductState {
    pub schema_version: u32,
    pub preferences: ViewProductPreferences,
    pub privacy: ViewPrivacySettings,
    pub window: ViewWindowState,
    pub recent_artifacts: Slice<ViewRecentArtifact>,
    pub workspace: ViewWorkspacePreferences,
}
impl Project for transmog_app::ProductState {
    type View = ViewProductState;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewProductState {
            schema_version: self.schema_version.project(arena)?,
            preferences: self.preferences.project(arena)?,
            privacy: self.privacy.project(arena)?,
            window: self.window.project(arena)?,
            recent_artifacts: self.recent_artifacts.project(arena)?,
            workspace: self.workspace.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::ProductState {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            schema_version: unsafe { FromAbi::read(&value.schema_version, budget) }?,
            preferences: unsafe { FromAbi::read(&value.preferences, budget) }?,
            privacy: unsafe { FromAbi::read(&value.privacy, budget) }?,
            window: unsafe { FromAbi::read(&value.window, budget) }?,
            recent_artifacts: unsafe { FromAbi::read(&value.recent_artifacts, budget) }?,
            workspace: unsafe { FromAbi::read(&value.workspace, budget) }?,
        })
    }
}
pub fn proxy_route_number(value: &transmog_app::ProxyRoute) -> u32 {
    match value {
        transmog_app::ProxyRoute::Auto => 0,
        transmog_app::ProxyRoute::Http1 => 1,
        transmog_app::ProxyRoute::Http2 => 2,
        transmog_app::ProxyRoute::Http3 => 3,
    }
}
impl Project for transmog_app::ProxyRoute {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(proxy_route_number(self))
    }
}
impl FromAbi for transmog_app::ProxyRoute {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Auto,
            1 => Self::Http1,
            2 => Self::Http2,
            3 => Self::Http3,
            _ => return Err(invalid("Invalid ProxyRoute discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewProxyStartRequest {
    pub ca_certificate_path: Text,
    pub ca_private_key_path: Text,
    pub listen: Text,
    pub route: u32,
    pub allow_remote_clients: u32,
    pub recording_only: u32,
}
impl Project for transmog_app::ProxyStartRequest {
    type View = ViewProxyStartRequest;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewProxyStartRequest {
            ca_certificate_path: self.ca_certificate_path.project(arena)?,
            ca_private_key_path: self.ca_private_key_path.project(arena)?,
            listen: self.listen.project(arena)?,
            route: self.route.project(arena)?,
            allow_remote_clients: self.allow_remote_clients.project(arena)?,
            recording_only: self.recording_only.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::ProxyStartRequest {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            ca_certificate_path: unsafe { FromAbi::read(&value.ca_certificate_path, budget) }?,
            ca_private_key_path: unsafe { FromAbi::read(&value.ca_private_key_path, budget) }?,
            listen: unsafe { FromAbi::read(&value.listen, budget) }?,
            route: unsafe { FromAbi::read(&value.route, budget) }?,
            allow_remote_clients: unsafe { FromAbi::read(&value.allow_remote_clients, budget) }?,
            recording_only: unsafe { FromAbi::read(&value.recording_only, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewIncidentWindow {
    pub trace_id: Text,
    pub marker_id: Text,
    pub before_seconds: u32,
    pub after_seconds: u32,
    pub relevant_hosts_only: u32,
}
impl Project for transmog_app::incidents::IncidentWindow {
    type View = ViewIncidentWindow;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewIncidentWindow {
            trace_id: self.trace_id.project(arena)?,
            marker_id: self.marker_id.project(arena)?,
            before_seconds: self.before_seconds.project(arena)?,
            after_seconds: self.after_seconds.project(arena)?,
            relevant_hosts_only: self.relevant_hosts_only.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::incidents::IncidentWindow {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            trace_id: unsafe { FromAbi::read(&value.trace_id, budget) }?,
            marker_id: unsafe { FromAbi::read(&value.marker_id, budget) }?,
            before_seconds: unsafe { FromAbi::read(&value.before_seconds, budget) }?,
            after_seconds: unsafe { FromAbi::read(&value.after_seconds, budget) }?,
            relevant_hosts_only: unsafe { FromAbi::read(&value.relevant_hosts_only, budget) }?,
        })
    }
}
pub fn filter_operator_number(value: &transmog_app::FilterOperator) -> u32 {
    match value {
        transmog_app::FilterOperator::Contains => 0,
        transmog_app::FilterOperator::Equals => 1,
        transmog_app::FilterOperator::Minimum => 2,
        transmog_app::FilterOperator::Maximum => 3,
    }
}
impl Project for transmog_app::FilterOperator {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(filter_operator_number(self))
    }
}
impl FromAbi for transmog_app::FilterOperator {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Contains,
            1 => Self::Equals,
            2 => Self::Minimum,
            3 => Self::Maximum,
            _ => return Err(invalid("Invalid FilterOperator discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionColumnFilter {
    pub column: u32,
    pub operator: u32,
    pub value: Text,
}
impl Project for transmog_app::SessionColumnFilter {
    type View = ViewSessionColumnFilter;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionColumnFilter {
            column: self.column.project(arena)?,
            operator: self.operator.project(arena)?,
            value: self.value.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::SessionColumnFilter {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            column: unsafe { FromAbi::read(&value.column, budget) }?,
            operator: unsafe { FromAbi::read(&value.operator, budget) }?,
            value: unsafe { FromAbi::read(&value.value, budget) }?,
        })
    }
}
pub fn traffic_facet_number(value: &transmog_app::TrafficFacet) -> u32 {
    match value {
        transmog_app::TrafficFacet::TrafficType => 0,
        transmog_app::TrafficFacet::FetchDestination => 1,
    }
}
impl Project for transmog_app::TrafficFacet {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(traffic_facet_number(self))
    }
}
impl FromAbi for transmog_app::TrafficFacet {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::TrafficType,
            1 => Self::FetchDestination,
            _ => return Err(invalid("Invalid TrafficFacet discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficDimensionColumn {
    pub item0: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficDimensionFacet {
    pub item0: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficDimension {
    pub kind: u32,
    pub column: ViewTrafficDimensionColumn,
    pub facet: ViewTrafficDimensionFacet,
}
impl Project for transmog_app::TrafficDimension {
    type View = ViewTrafficDimension;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(match self {
            Self::Column(item0) => ViewTrafficDimension {
                kind: 0,
                column: ViewTrafficDimensionColumn {
                    item0: item0.project(arena)?,
                },
                ..Default::default()
            },
            Self::Facet(item0) => ViewTrafficDimension {
                kind: 1,
                facet: ViewTrafficDimensionFacet {
                    item0: item0.project(arena)?,
                },
                ..Default::default()
            },
        })
    }
}
impl FromAbi for transmog_app::TrafficDimension {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match value.kind {
            0 => Self::Column(unsafe { FromAbi::read(&value.column.item0, budget) }?),
            1 => Self::Facet(unsafe { FromAbi::read(&value.facet.item0, budget) }?),
            _ => return Err(invalid("Invalid TrafficDimension discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficFilterValueText {
    pub item0: Text,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficFilterValueNumber {
    pub item0: Text,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficFilterValueStatusFamily {
    pub item0: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficFilterValue {
    pub kind: u32,
    pub text: ViewTrafficFilterValueText,
    pub number: ViewTrafficFilterValueNumber,
    pub status_family: ViewTrafficFilterValueStatusFamily,
}
impl Project for transmog_app::TrafficFilterValue {
    type View = ViewTrafficFilterValue;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(match self {
            Self::Text(item0) => ViewTrafficFilterValue {
                kind: 0,
                text: ViewTrafficFilterValueText {
                    item0: item0.project(arena)?,
                },
                ..Default::default()
            },
            Self::Number(item0) => ViewTrafficFilterValue {
                kind: 1,
                number: ViewTrafficFilterValueNumber {
                    item0: item0.project(arena)?,
                },
                ..Default::default()
            },
            Self::StatusFamily(item0) => ViewTrafficFilterValue {
                kind: 2,
                status_family: ViewTrafficFilterValueStatusFamily {
                    item0: item0.project(arena)?,
                },
                ..Default::default()
            },
            Self::Unavailable => ViewTrafficFilterValue {
                kind: 3,
                ..Default::default()
            },
            Self::Other => ViewTrafficFilterValue {
                kind: 4,
                ..Default::default()
            },
        })
    }
}
impl FromAbi for transmog_app::TrafficFilterValue {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match value.kind {
            0 => Self::Text(unsafe { FromAbi::read(&value.text.item0, budget) }?),
            1 => Self::Number(unsafe { FromAbi::read(&value.number.item0, budget) }?),
            2 => Self::StatusFamily(unsafe { FromAbi::read(&value.status_family.item0, budget) }?),
            3 => Self::Unavailable,
            4 => Self::Other,
            _ => return Err(invalid("Invalid TrafficFilterValue discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficFilterGroup {
    pub dimension: ViewTrafficDimension,
    pub values: Slice<ViewTrafficFilterValue>,
}
impl Project for transmog_app::TrafficFilterGroup {
    type View = ViewTrafficFilterGroup;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewTrafficFilterGroup {
            dimension: self.dimension.project(arena)?,
            values: self.values.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::TrafficFilterGroup {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            dimension: unsafe { FromAbi::read(&value.dimension, budget) }?,
            values: unsafe { FromAbi::read(&value.values, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClassificationFilter {
    pub column: Text,
    pub values: Slice<ViewTrafficFilterValue>,
}
impl Project for transmog_app::ClassificationFilter {
    type View = ViewClassificationFilter;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewClassificationFilter {
            column: self.column.project(arena)?,
            values: self.values.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::ClassificationFilter {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            column: unsafe { FromAbi::read(&value.column, budget) }?,
            values: unsafe { FromAbi::read(&value.values, budget) }?,
        })
    }
}
pub fn sort_direction_number(value: &transmog_app::SortDirection) -> u32 {
    match value {
        transmog_app::SortDirection::Ascending => 0,
        transmog_app::SortDirection::Descending => 1,
    }
}
impl Project for transmog_app::SortDirection {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(sort_direction_number(self))
    }
}
impl FromAbi for transmog_app::SortDirection {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(match *value {
            0 => Self::Ascending,
            1 => Self::Descending,
            _ => return Err(invalid("Invalid SortDirection discriminant")),
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionSort {
    pub column: u32,
    pub direction: u32,
}
impl Project for transmog_app::SessionSort {
    type View = ViewSessionSort;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionSort {
            column: self.column.project(arena)?,
            direction: self.direction.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::SessionSort {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            column: unsafe { FromAbi::read(&value.column, budget) }?,
            direction: unsafe { FromAbi::read(&value.direction, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionQueryInput {
    pub incident_window: Optional<ViewIncidentWindow>,
    pub search_result_id: Optional<Text>,
    pub cursor: Optional<Text>,
    pub latest: u32,
    pub limit: Optional<usize>,
    pub terminal: Optional<u32>,
    pub method: Optional<Text>,
    pub host: Optional<Text>,
    pub search: Optional<Text>,
    pub filters: Slice<ViewSessionColumnFilter>,
    pub filter_groups: Slice<ViewTrafficFilterGroup>,
    pub extension_filters: Slice<ViewClassificationFilter>,
    pub fetch_destinations: Slice<Text>,
    pub sort: Optional<ViewSessionSort>,
    pub offset: Optional<usize>,
    pub focus_id: Optional<Text>,
}
impl Project for transmog_app::SessionQueryInput {
    type View = ViewSessionQueryInput;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionQueryInput {
            incident_window: self.incident_window.project(arena)?,
            search_result_id: self.search_result_id.project(arena)?,
            cursor: self.cursor.project(arena)?,
            latest: self.latest.project(arena)?,
            limit: self.limit.project(arena)?,
            terminal: self.terminal.project(arena)?,
            method: self.method.project(arena)?,
            host: self.host.project(arena)?,
            search: self.search.project(arena)?,
            filters: self.filters.project(arena)?,
            filter_groups: self.filter_groups.project(arena)?,
            extension_filters: self.extension_filters.project(arena)?,
            fetch_destinations: self.fetch_destinations.project(arena)?,
            sort: self.sort.project(arena)?,
            offset: self.offset.project(arena)?,
            focus_id: self.focus_id.project(arena)?,
        })
    }
}
impl FromAbi for transmog_app::SessionQueryInput {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let _ = &budget;
        Ok(Self {
            incident_window: unsafe { FromAbi::read(&value.incident_window, budget) }?,
            search_result_id: unsafe { FromAbi::read(&value.search_result_id, budget) }?,
            cursor: unsafe { FromAbi::read(&value.cursor, budget) }?,
            latest: unsafe { FromAbi::read(&value.latest, budget) }?,
            limit: unsafe { FromAbi::read(&value.limit, budget) }?,
            terminal: unsafe { FromAbi::read(&value.terminal, budget) }?,
            method: unsafe { FromAbi::read(&value.method, budget) }?,
            host: unsafe { FromAbi::read(&value.host, budget) }?,
            search: unsafe { FromAbi::read(&value.search, budget) }?,
            filters: unsafe { FromAbi::read(&value.filters, budget) }?,
            filter_groups: unsafe { FromAbi::read(&value.filter_groups, budget) }?,
            extension_filters: unsafe { FromAbi::read(&value.extension_filters, budget) }?,
            fetch_destinations: unsafe { FromAbi::read(&value.fetch_destinations, budget) }?,
            sort: unsafe { FromAbi::read(&value.sort, budget) }?,
            offset: unsafe { FromAbi::read(&value.offset, budget) }?,
            focus_id: unsafe { FromAbi::read(&value.focus_id, budget) }?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewClientIdentityView {
    pub kind: Text,
    pub process_name: Optional<Text>,
    pub process_id: Optional<u32>,
}
impl Project for transmog_app::ClientIdentityView {
    type View = ViewClientIdentityView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewClientIdentityView {
            kind: self.kind.project(arena)?,
            process_name: self.process_name.project(arena)?,
            process_id: self.process_id.project(arena)?,
        })
    }
}
pub fn stream_protocol_number(value: &transmog_core::stream::StreamProtocol) -> u32 {
    match value {
        transmog_core::stream::StreamProtocol::WebSocket => 0,
        transmog_core::stream::StreamProtocol::Sse => 1,
    }
}
impl Project for transmog_core::stream::StreamProtocol {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(stream_protocol_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewStreamLifetime {
    pub duration_ms: Optional<u64>,
    pub established_at: Optional<u64>,
    pub ended_at: Optional<u64>,
    pub observed_until: Optional<u64>,
    pub live: u32,
    pub boundary: Text,
}
impl Project for transmog_app::StreamLifetime {
    type View = ViewStreamLifetime;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewStreamLifetime {
            duration_ms: self.duration_ms.project(arena)?,
            established_at: self.established_at.project(arena)?,
            ended_at: self.ended_at.project(arena)?,
            observed_until: self.observed_until.project(arena)?,
            live: self.live.project(arena)?,
            boundary: self.boundary.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewLocalResponseSnapshot {
    pub status: u16,
    pub body_bytes: u64,
    pub sha256: Text,
    pub content_codings: Slice<Text>,
}
impl Project for transmog_core::intercept::LocalResponseSnapshot {
    type View = ViewLocalResponseSnapshot;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewLocalResponseSnapshot {
            status: self.status.project(arena)?,
            body_bytes: self.body_bytes.project(arena)?,
            sha256: self.sha256.project(arena)?,
            content_codings: self.content_codings.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewAutoResponseMatchView {
    pub rule_id: Text,
    pub rule_name: Text,
    pub rule_revision: u64,
    pub position: usize,
    pub asset_reference: Text,
    pub status: u16,
    pub body_bytes: usize,
    pub snapshot: Optional<ViewLocalResponseSnapshot>,
}
impl Project for transmog_app::AutoResponseMatchView {
    type View = ViewAutoResponseMatchView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewAutoResponseMatchView {
            rule_id: self.rule_id.project(arena)?,
            rule_name: self.rule_name.project(arena)?,
            rule_revision: self.rule_revision.project(arena)?,
            position: self.position.project(arena)?,
            asset_reference: self.asset_reference.project(arena)?,
            status: self.status.project(arena)?,
            body_bytes: self.body_bytes.project(arena)?,
            snapshot: self.snapshot.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionSummary {
    pub request_number: Text,
    pub id: Text,
    pub caller: ViewClientIdentityView,
    pub method: Text,
    pub host: Text,
    pub path: Text,
    pub protocol: Text,
    pub stream_protocol: Optional<u32>,
    pub status: Optional<u16>,
    pub duration_ms: Optional<u64>,
    pub http_duration_ms: Optional<u64>,
    pub response_headers_ms: Optional<u64>,
    pub stream_lifetime: Optional<ViewStreamLifetime>,
    pub trace_id: Optional<Text>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub stream_sent: Optional<u64>,
    pub stream_received: Optional<u64>,
    pub terminal: Text,
    pub loss: u32,
    pub capturing: u32,
    pub auto_response: Optional<ViewAutoResponseMatchView>,
    pub url: Text,
    pub started_at: u64,
    pub content_type: Optional<Text>,
    pub top_level_navigation: u32,
    pub fetch_destination: Optional<Text>,
}
impl Project for transmog_app::SessionSummary {
    type View = ViewSessionSummary;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionSummary {
            request_number: self.request_number.project(arena)?,
            id: self.id.project(arena)?,
            caller: self.caller.project(arena)?,
            method: self.method.project(arena)?,
            host: self.host.project(arena)?,
            path: self.path.project(arena)?,
            protocol: self.protocol.project(arena)?,
            stream_protocol: self.stream_protocol.project(arena)?,
            status: self.status.project(arena)?,
            duration_ms: self.duration_ms.project(arena)?,
            http_duration_ms: self.http_duration_ms.project(arena)?,
            response_headers_ms: self.response_headers_ms.project(arena)?,
            stream_lifetime: self.stream_lifetime.project(arena)?,
            trace_id: self.trace_id.project(arena)?,
            request_bytes: self.request_bytes.project(arena)?,
            response_bytes: self.response_bytes.project(arena)?,
            stream_sent: self.stream_sent.project(arena)?,
            stream_received: self.stream_received.project(arena)?,
            terminal: self.terminal.project(arena)?,
            loss: self.loss.project(arena)?,
            capturing: self.capturing.project(arena)?,
            auto_response: self.auto_response.project(arena)?,
            url: self.url.project(arena)?,
            started_at: self.started_at.project(arena)?,
            content_type: self.content_type.project(arena)?,
            top_level_navigation: self.top_level_navigation.project(arena)?,
            fetch_destination: self.fetch_destination.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionPage {
    pub sessions: Slice<ViewSessionSummary>,
    pub next_cursor: Optional<Text>,
    pub evicted: u64,
    pub sequence_gaps: u64,
    pub subscriber_lag: u64,
    pub total_matched: usize,
    pub retained_count: usize,
    pub focus_offset: Optional<usize>,
}
impl Project for transmog_app::SessionPage {
    type View = ViewSessionPage;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionPage {
            sessions: self.sessions.project(arena)?,
            next_cursor: self.next_cursor.project(arena)?,
            evicted: self.evicted.project(arena)?,
            sequence_gaps: self.sequence_gaps.project(arena)?,
            subscriber_lag: self.subscriber_lag.project(arena)?,
            total_matched: self.total_matched.project(arena)?,
            retained_count: self.retained_count.project(arena)?,
            focus_offset: self.focus_offset.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTrafficView {
    pub id: Text,
    pub ids: Slice<Text>,
    pub page: ViewSessionPage,
}
impl Project for transmog_app::TrafficView {
    type View = ViewTrafficView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewTrafficView {
            id: self.id.project(arena)?,
            ids: self.ids.project(arena)?,
            page: self.page.project(arena)?,
        })
    }
}
pub fn stream_status_number(value: &transmog_core::stream::StreamStatus) -> u32 {
    match value {
        transmog_core::stream::StreamStatus::Connecting => 0,
        transmog_core::stream::StreamStatus::Open => 1,
        transmog_core::stream::StreamStatus::Closing => 2,
        transmog_core::stream::StreamStatus::Closed => 3,
        transmog_core::stream::StreamStatus::Failed => 4,
        transmog_core::stream::StreamStatus::Interrupted => 5,
    }
}
impl Project for transmog_core::stream::StreamStatus {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(stream_status_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewStreamState {
    pub protocol: u32,
    pub status: u32,
    pub at_millis: u64,
    pub reason: Optional<Text>,
}
impl Project for transmog_core::stream::StreamState {
    type View = ViewStreamState;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewStreamState {
            protocol: self.protocol.project(arena)?,
            status: self.status.project(arena)?,
            at_millis: self.at_millis.project(arena)?,
            reason: self.reason.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewStreamSummary {
    pub state: ViewStreamState,
    pub imported: u32,
    pub revision: u64,
    pub observed: u64,
    pub evicted: u64,
    pub sequence_loss: u64,
    pub retained: usize,
    pub client_bytes: Optional<u64>,
    pub server_bytes: Optional<u64>,
    pub opened_at: Optional<u64>,
    pub last_activity: Optional<u64>,
    pub lifetime: ViewStreamLifetime,
    pub http_duration_ms: Optional<u64>,
    pub response_headers_ms: Optional<u64>,
}
impl Project for transmog_app::StreamSummary {
    type View = ViewStreamSummary;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewStreamSummary {
            state: self.state.project(arena)?,
            imported: self.imported.project(arena)?,
            revision: self.revision.project(arena)?,
            observed: self.observed.project(arena)?,
            evicted: self.evicted.project(arena)?,
            sequence_loss: self.sequence_loss.project(arena)?,
            retained: self.retained.project(arena)?,
            client_bytes: self.client_bytes.project(arena)?,
            server_bytes: self.server_bytes.project(arena)?,
            opened_at: self.opened_at.project(arena)?,
            last_activity: self.last_activity.project(arena)?,
            lifetime: self.lifetime.project(arena)?,
            http_duration_ms: self.http_duration_ms.project(arena)?,
            response_headers_ms: self.response_headers_ms.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewComposerOrigin {
    pub entry_id: Text,
    pub trace_id: Optional<Text>,
    pub trace_name: Optional<Text>,
    pub original_id: Optional<Text>,
}
impl Project for transmog_app::ComposerOrigin {
    type View = ViewComposerOrigin;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewComposerOrigin {
            entry_id: self.entry_id.project(arena)?,
            trace_id: self.trace_id.project(arena)?,
            trace_name: self.trace_name.project(arena)?,
            original_id: self.original_id.project(arena)?,
        })
    }
}
pub fn milestone_number(value: &transmog_core::performance::Milestone) -> u32 {
    match value {
        transmog_core::performance::Milestone::ClientConnected => 0,
        transmog_core::performance::Milestone::ClientFirstByte => 1,
        transmog_core::performance::Milestone::ClientIdentityDone => 2,
        transmog_core::performance::Milestone::ClientTlsBegin => 3,
        transmog_core::performance::Milestone::ClientTlsDone => 4,
        transmog_core::performance::Milestone::RequestHeaders => 5,
        transmog_core::performance::Milestone::ClientRequestDone => 6,
        transmog_core::performance::Milestone::RouteBegin => 7,
        transmog_core::performance::Milestone::RouteDone => 8,
        transmog_core::performance::Milestone::UpstreamBegin => 9,
        transmog_core::performance::Milestone::UpstreamConnected => 10,
        transmog_core::performance::Milestone::UpstreamRequestConsumed => 11,
        transmog_core::performance::Milestone::UpstreamRequestFirstBodyQueued => 12,
        transmog_core::performance::Milestone::UpstreamResponseFirstBody => 13,
        transmog_core::performance::Milestone::ClientResponseFirstBodyQueued => 14,
        transmog_core::performance::Milestone::ResponseHeaders => 15,
        transmog_core::performance::Milestone::UpstreamResponseDone => 16,
        transmog_core::performance::Milestone::ClientResponseBegin => 17,
        transmog_core::performance::Milestone::ClientResponseQueued => 18,
        transmog_core::performance::Milestone::ExchangeDone => 19,
    }
}
impl Project for transmog_core::performance::Milestone {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(milestone_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTimingPoint {
    pub milestone: u32,
    pub unix_millis: u64,
    pub offset_micros: i64,
}
impl Project for transmog_core::performance::TimingPoint {
    type View = ViewTimingPoint;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewTimingPoint {
            milestone: self.milestone.project(arena)?,
            unix_millis: self.unix_millis.project(arena)?,
            offset_micros: self.offset_micros.project(arena)?,
        })
    }
}
pub fn transport_outcome_number(value: &transmog_core::performance::TransportOutcome) -> u32 {
    match value {
        transmog_core::performance::TransportOutcome::PolicyRejected => 0,
        transmog_core::performance::TransportOutcome::Unavailable => 1,
        transmog_core::performance::TransportOutcome::Connected => 2,
        transmog_core::performance::TransportOutcome::DnsFailed => 3,
        transmog_core::performance::TransportOutcome::TcpFailed => 4,
        transmog_core::performance::TransportOutcome::TlsFailed => 5,
    }
}
impl Project for transmog_core::performance::TransportOutcome {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(transport_outcome_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewConnectionTiming {
    pub phase: Text,
    pub began_offset_micros: i64,
    pub ended_offset_micros: i64,
    pub request_wait_micros: u64,
}
impl Project for transmog_core::performance::ConnectionTiming {
    type View = ViewConnectionTiming;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewConnectionTiming {
            phase: self.phase.project(arena)?,
            began_offset_micros: self.began_offset_micros.project(arena)?,
            ended_offset_micros: self.ended_offset_micros.project(arena)?,
            request_wait_micros: self.request_wait_micros.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewAlpsEvidence {
    pub negotiated: u32,
    pub peer_settings: Slice<u8>,
}
impl Project for transmog_core::AlpsEvidence {
    type View = ViewAlpsEvidence;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewAlpsEvidence {
            negotiated: self.negotiated.project(arena)?,
            peer_settings: self.peer_settings.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewPublicCertificate {
    pub sha256: Text,
    pub der: Slice<u8>,
}
impl Project for transmog_core::PublicCertificate {
    type View = ViewPublicCertificate;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewPublicCertificate {
            sha256: self.sha256.project(arena)?,
            der: self.der.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewVerificationResultRejected {
    pub error_code: i32,
    pub depth: Optional<u32>,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewVerificationResult {
    pub kind: u32,
    pub rejected: ViewVerificationResultRejected,
}
impl Project for transmog_core::VerificationResult {
    type View = ViewVerificationResult;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(match self {
            Self::Verified => ViewVerificationResult {
                kind: 0,
                ..Default::default()
            },
            Self::Rejected { error_code, depth } => ViewVerificationResult {
                kind: 1,
                rejected: ViewVerificationResultRejected {
                    error_code: error_code.project(arena)?,
                    depth: depth.project(arena)?,
                },
            },
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTlsEvidence {
    pub http3_alps: Optional<ViewAlpsEvidence>,
    pub http2_alps: Optional<ViewAlpsEvidence>,
    pub network_profile: Optional<ViewNetworkProfileSnapshot>,
    pub server_name: Optional<Text>,
    pub server_chain: Slice<ViewPublicCertificate>,
    pub client_chain: Slice<ViewPublicCertificate>,
    pub chain_source: Optional<Text>,
    pub offered_versions: Slice<Text>,
    pub offered_ciphers: Slice<Text>,
    pub offered_alpn: Slice<Text>,
    pub trust_source: Optional<Text>,
    pub trust_generation: Optional<u64>,
    pub verification: Optional<ViewVerificationResult>,
    pub errors: Slice<Text>,
    pub details: Slice<TextEntry>,
}
impl Project for transmog_core::TlsEvidence {
    type View = ViewTlsEvidence;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewTlsEvidence {
            http3_alps: self.http3_alps.project(arena)?,
            http2_alps: self.http2_alps.project(arena)?,
            network_profile: self.network_profile.project(arena)?,
            server_name: self.server_name.project(arena)?,
            server_chain: self.server_chain.project(arena)?,
            client_chain: self.client_chain.project(arena)?,
            chain_source: self.chain_source.project(arena)?,
            offered_versions: self.offered_versions.project(arena)?,
            offered_ciphers: self.offered_ciphers.project(arena)?,
            offered_alpn: self.offered_alpn.project(arena)?,
            trust_source: self.trust_source.project(arena)?,
            trust_generation: self.trust_generation.project(arena)?,
            verification: self.verification.project(arena)?,
            errors: self.errors.project(arena)?,
            details: self.details.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTcpObservation {
    pub rtt_micros: Optional<u64>,
    pub min_rtt_micros: Optional<u64>,
    pub congestion_window: Optional<u64>,
    pub send_window: Optional<u64>,
    pub receive_window: Optional<u64>,
    pub unacknowledged_bytes: Optional<u64>,
    pub retransmitted_bytes: Optional<u64>,
    pub retransmitted_segments: Optional<u64>,
    pub fast_retransmissions: Optional<u64>,
    pub duplicate_acks: Optional<u64>,
    pub timeout_episodes: Optional<u64>,
    pub mss: Optional<u64>,
    pub connection_age_millis: Optional<u64>,
}
impl Project for transmog_core::performance::TcpObservation {
    type View = ViewTcpObservation;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewTcpObservation {
            rtt_micros: self.rtt_micros.project(arena)?,
            min_rtt_micros: self.min_rtt_micros.project(arena)?,
            congestion_window: self.congestion_window.project(arena)?,
            send_window: self.send_window.project(arena)?,
            receive_window: self.receive_window.project(arena)?,
            unacknowledged_bytes: self.unacknowledged_bytes.project(arena)?,
            retransmitted_bytes: self.retransmitted_bytes.project(arena)?,
            retransmitted_segments: self.retransmitted_segments.project(arena)?,
            fast_retransmissions: self.fast_retransmissions.project(arena)?,
            duplicate_acks: self.duplicate_acks.project(arena)?,
            timeout_episodes: self.timeout_episodes.project(arena)?,
            mss: self.mss.project(arena)?,
            connection_age_millis: self.connection_age_millis.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSocketIoObservation {
    pub write_wait_micros: u64,
    pub write_waits: u64,
    pub last_write_offset_micros: Optional<i64>,
    pub last_flush_offset_micros: Optional<i64>,
}
impl Project for transmog_core::performance::SocketIoObservation {
    type View = ViewSocketIoObservation;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSocketIoObservation {
            write_wait_micros: self.write_wait_micros.project(arena)?,
            write_waits: self.write_waits.project(arena)?,
            last_write_offset_micros: self.last_write_offset_micros.project(arena)?,
            last_flush_offset_micros: self.last_flush_offset_micros.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewQuicObservation {
    pub rtt_micros: Optional<u64>,
    pub congestion_window: Optional<u64>,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub retransmitted_bytes: u64,
}
impl Project for transmog_core::performance::QuicObservation {
    type View = ViewQuicObservation;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewQuicObservation {
            rtt_micros: self.rtt_micros.project(arena)?,
            congestion_window: self.congestion_window.project(arena)?,
            packets_sent: self.packets_sent.project(arena)?,
            packets_received: self.packets_received.project(arena)?,
            packets_lost: self.packets_lost.project(arena)?,
            retransmitted_bytes: self.retransmitted_bytes.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewTransportObservation {
    pub tunnel_destination: Optional<Text>,
    pub sampled_offset_micros: i64,
    pub outcome: u32,
    pub leg: Text,
    pub connection_id: Text,
    pub peer: Optional<Text>,
    pub local: Optional<Text>,
    pub shared: u32,
    pub setup_timings: Slice<ViewConnectionTiming>,
    pub dns_micros: Optional<u64>,
    pub tcp_micros: Optional<u64>,
    pub tls_micros: Optional<u64>,
    pub tls_version: Optional<Text>,
    pub tls_resumed: Optional<u32>,
    pub tls: Optional<ViewTlsEvidence>,
    pub cipher: Optional<Text>,
    pub alpn: Optional<Text>,
    pub bytes_read: Optional<u64>,
    pub bytes_written: Optional<u64>,
    pub tcp: Optional<ViewTcpObservation>,
    pub tcp_sampled_offset_micros: Optional<i64>,
    pub socket_io: Optional<ViewSocketIoObservation>,
    pub quic: Optional<ViewQuicObservation>,
}
impl Project for transmog_core::performance::TransportObservation {
    type View = ViewTransportObservation;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewTransportObservation {
            tunnel_destination: self.tunnel_destination.project(arena)?,
            sampled_offset_micros: self.sampled_offset_micros.project(arena)?,
            outcome: self.outcome.project(arena)?,
            leg: self.leg.project(arena)?,
            connection_id: self.connection_id.project(arena)?,
            peer: self.peer.project(arena)?,
            local: self.local.project(arena)?,
            shared: self.shared.project(arena)?,
            setup_timings: self.setup_timings.project(arena)?,
            dns_micros: self.dns_micros.project(arena)?,
            tcp_micros: self.tcp_micros.project(arena)?,
            tls_micros: self.tls_micros.project(arena)?,
            tls_version: self.tls_version.project(arena)?,
            tls_resumed: self.tls_resumed.project(arena)?,
            tls: self.tls.project(arena)?,
            cipher: self.cipher.project(arena)?,
            alpn: self.alpn.project(arena)?,
            bytes_read: self.bytes_read.project(arena)?,
            bytes_written: self.bytes_written.project(arena)?,
            tcp: self.tcp.project(arena)?,
            tcp_sampled_offset_micros: self.tcp_sampled_offset_micros.project(arena)?,
            socket_io: self.socket_io.project(arena)?,
            quic: self.quic.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewProtocolObservation {
    pub boundary: Text,
    pub version: Text,
    pub reason: Optional<Text>,
}
impl Project for transmog_core::performance::ProtocolObservation {
    type View = ViewProtocolObservation;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewProtocolObservation {
            boundary: self.boundary.project(arena)?,
            version: self.version.project(arena)?,
            reason: self.reason.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewWorkTiming {
    pub kind: Text,
    pub label: Text,
    pub began_offset_micros: i64,
    pub ended_offset_micros: i64,
    pub busy_nanos: u64,
    pub calls: u64,
}
impl Project for transmog_core::performance::WorkTiming {
    type View = ViewWorkTiming;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewWorkTiming {
            kind: self.kind.project(arena)?,
            label: self.label.project(arena)?,
            began_offset_micros: self.began_offset_micros.project(arena)?,
            ended_offset_micros: self.ended_offset_micros.project(arena)?,
            busy_nanos: self.busy_nanos.project(arena)?,
            calls: self.calls.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewPerformanceEvidence {
    pub points: Slice<ViewTimingPoint>,
    pub transports: Slice<ViewTransportObservation>,
    pub protocols: Slice<ViewProtocolObservation>,
    pub work: Slice<ViewWorkTiming>,
}
impl Project for transmog_core::performance::PerformanceEvidence {
    type View = ViewPerformanceEvidence;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewPerformanceEvidence {
            points: self.points.project(arena)?,
            transports: self.transports.project(arena)?,
            protocols: self.protocols.project(arena)?,
            work: self.work.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewHeaderSummary {
    pub total_fields: usize,
    pub value_bytes: Optional<u64>,
    pub serialized_bytes: Optional<u64>,
    pub authorization: Text,
    pub proxy_authorization: Text,
}
impl Project for transmog_app::HeaderSummary {
    type View = ViewHeaderSummary;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewHeaderSummary {
            total_fields: self.total_fields.project(arena)?,
            value_bytes: self.value_bytes.project(arena)?,
            serialized_bytes: self.serialized_bytes.project(arena)?,
            authorization: self.authorization.project(arena)?,
            proxy_authorization: self.proxy_authorization.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewHeaderView {
    pub index: usize,
    pub value_bytes: Optional<usize>,
    pub field_bytes: Optional<usize>,
    pub name: Text,
    pub value: Text,
    pub binary: u32,
    pub sensitive: u32,
}
impl Project for transmog_app::HeaderView {
    type View = ViewHeaderView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewHeaderView {
            index: self.index.project(arena)?,
            value_bytes: self.value_bytes.project(arena)?,
            field_bytes: self.field_bytes.project(arena)?,
            name: self.name.project(arena)?,
            value: self.value.project(arena)?,
            binary: self.binary.project(arena)?,
            sensitive: self.sensitive.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewHeadView {
    pub summary: ViewHeaderSummary,
    pub boundary: Text,
    pub method: Optional<Text>,
    pub target: Optional<Text>,
    pub status: Optional<u16>,
    pub protocol: Text,
    pub headers: Slice<ViewHeaderView>,
}
impl Project for transmog_app::HeadView {
    type View = ViewHeadView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewHeadView {
            summary: self.summary.project(arena)?,
            boundary: self.boundary.project(arena)?,
            method: self.method.project(arena)?,
            target: self.target.project(arena)?,
            status: self.status.project(arena)?,
            protocol: self.protocol.project(arena)?,
            headers: self.headers.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewBodyView {
    pub boundary: Text,
    pub observed_bytes: u64,
    pub display: Text,
    pub representation: Text,
    pub truncated: u32,
    pub trailers: Slice<ViewHeaderView>,
}
impl Project for transmog_app::BodyView {
    type View = ViewBodyView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewBodyView {
            boundary: self.boundary.project(arena)?,
            observed_bytes: self.observed_bytes.project(arena)?,
            display: self.display.project(arena)?,
            representation: self.representation.project(arena)?,
            truncated: self.truncated.project(arena)?,
            trailers: self.trailers.project(arena)?,
        })
    }
}
pub fn body_availability_number(value: &transmog_app::BodyAvailability) -> u32 {
    match value {
        transmog_app::BodyAvailability::Capturing => 1,
        transmog_app::BodyAvailability::Complete => 2,
        transmog_app::BodyAvailability::Truncated => 3,
        transmog_app::BodyAvailability::Lost => 4,
        transmog_app::BodyAvailability::Evicted => 5,
        transmog_app::BodyAvailability::Disabled => 6,
        transmog_app::BodyAvailability::QuotaOmitted => 7,
    }
}
impl Project for transmog_app::BodyAvailability {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(body_availability_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewStoredBodyMetadata {
    pub length_known: u32,
    pub exchange_id: Text,
    pub boundary: Text,
    pub observed_bytes: u64,
    pub wire_body_bytes: Optional<u64>,
    pub retained_bytes: u64,
    pub availability: u32,
    pub media_type: Optional<Text>,
    pub charset: Optional<Text>,
    pub content_codings: Slice<Text>,
    pub sha256: Optional<Text>,
    pub reason: Optional<Text>,
}
impl Project for transmog_app::StoredBodyMetadata {
    type View = ViewStoredBodyMetadata;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewStoredBodyMetadata {
            length_known: self.length_known.project(arena)?,
            exchange_id: self.exchange_id.project(arena)?,
            boundary: self.boundary.project(arena)?,
            observed_bytes: self.observed_bytes.project(arena)?,
            wire_body_bytes: self.wire_body_bytes.project(arena)?,
            retained_bytes: self.retained_bytes.project(arena)?,
            availability: self.availability.project(arena)?,
            media_type: self.media_type.project(arena)?,
            charset: self.charset.project(arena)?,
            content_codings: self.content_codings.project(arena)?,
            sha256: self.sha256.project(arena)?,
            reason: self.reason.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewHeaderChanges {
    pub added: Slice<Text>,
    pub removed: Slice<Text>,
    pub changed: Slice<Text>,
}
impl Project for transmog_core::intercept::HeaderChanges {
    type View = ViewHeaderChanges;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewHeaderChanges {
            added: self.added.project(arena)?,
            removed: self.removed.project(arena)?,
            changed: self.changed.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewHeaderEffectView {
    pub sequence: u64,
    pub hook_id: Text,
    pub name: Text,
    pub phase: Text,
    pub changed: u32,
    pub headers: ViewHeaderChanges,
}
impl Project for transmog_app::HeaderEffectView {
    type View = ViewHeaderEffectView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewHeaderEffectView {
            sequence: self.sequence.project(arena)?,
            hook_id: self.hook_id.project(arena)?,
            name: self.name.project(arena)?,
            phase: self.phase.project(arena)?,
            changed: self.changed.project(arena)?,
            headers: self.headers.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewBodyEditReport {
    pub outcome: Text,
    pub matches: Optional<usize>,
    pub replacements: usize,
    pub before_bytes: Optional<usize>,
    pub after_bytes: Optional<usize>,
    pub before_sha256: Optional<Text>,
    pub after_sha256: Optional<Text>,
}
impl Project for transmog_core::intercept::BodyEditReport {
    type View = ViewBodyEditReport;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewBodyEditReport {
            outcome: self.outcome.project(arena)?,
            matches: self.matches.project(arena)?,
            replacements: self.replacements.project(arena)?,
            before_bytes: self.before_bytes.project(arena)?,
            after_bytes: self.after_bytes.project(arena)?,
            before_sha256: self.before_sha256.project(arena)?,
            after_sha256: self.after_sha256.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewBodyEditView {
    pub sequence: u64,
    pub hook_id: Text,
    pub name: Text,
    pub report: ViewBodyEditReport,
}
impl Project for transmog_app::BodyEditView {
    type View = ViewBodyEditView;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewBodyEditView {
            sequence: self.sequence.project(arena)?,
            hook_id: self.hook_id.project(arena)?,
            name: self.name.project(arena)?,
            report: self.report.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionDetail {
    pub auth_revisions: Slice<TextEntry>,
    pub request_number: Text,
    pub original_request_number: Optional<Text>,
    pub original_request_number_provenance: Optional<Text>,
    pub stream: Optional<ViewStreamSummary>,
    pub composer_origin: Optional<ViewComposerOrigin>,
    pub performance: ViewPerformanceEvidence,
    pub trace_id: Optional<Text>,
    pub original_id: Optional<Text>,
    pub saved_evidence: Slice<TextEntry>,
    pub source_ip: Optional<Text>,
    pub id: Text,
    pub started_at: u64,
    pub caller: ViewClientIdentityView,
    pub requests: Slice<ViewHeadView>,
    pub responses: Slice<ViewHeadView>,
    pub bodies: Slice<ViewBodyView>,
    pub stored_bodies: Slice<ViewStoredBodyMetadata>,
    pub diagnostics: Slice<Text>,
    pub hook_effects: Slice<Text>,
    pub header_effects: Slice<ViewHeaderEffectView>,
    pub body_edits: Slice<ViewBodyEditView>,
    pub auto_response: Optional<ViewAutoResponseMatchView>,
    pub route_selection: Optional<Text>,
    pub route_attempts: Slice<Text>,
    pub terminal: Text,
    pub terminal_state: Text,
    pub websocket: Optional<Text>,
    pub sequence_loss: u64,
}
impl Project for transmog_app::SessionDetail {
    type View = ViewSessionDetail;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionDetail {
            auth_revisions: self.auth_revisions.project(arena)?,
            request_number: self.request_number.project(arena)?,
            original_request_number: self.original_request_number.project(arena)?,
            original_request_number_provenance: self
                .original_request_number_provenance
                .project(arena)?,
            stream: self.stream.project(arena)?,
            composer_origin: self.composer_origin.project(arena)?,
            performance: self.performance.project(arena)?,
            trace_id: self.trace_id.project(arena)?,
            original_id: self.original_id.project(arena)?,
            saved_evidence: self.saved_evidence.project(arena)?,
            source_ip: self.source_ip.project(arena)?,
            id: self.id.project(arena)?,
            started_at: self.started_at.project(arena)?,
            caller: self.caller.project(arena)?,
            requests: self.requests.project(arena)?,
            responses: self.responses.project(arena)?,
            bodies: self.bodies.project(arena)?,
            stored_bodies: self.stored_bodies.project(arena)?,
            diagnostics: self.diagnostics.project(arena)?,
            hook_effects: self.hook_effects.project(arena)?,
            header_effects: self.header_effects.project(arena)?,
            body_edits: self.body_edits.project(arena)?,
            auto_response: self.auto_response.project(arena)?,
            route_selection: self.route_selection.project(arena)?,
            route_attempts: self.route_attempts.project(arena)?,
            terminal: self.terminal.project(arena)?,
            terminal_state: self.terminal_state.project(arena)?,
            websocket: self.websocket.project(arena)?,
            sequence_loss: self.sequence_loss.project(arena)?,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewSessionHint {
    pub exchange_id: Optional<Text>,
    pub sequence: u64,
    pub lagged: u32,
}
impl Project for transmog_app::SessionHint {
    type View = ViewSessionHint;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewSessionHint {
            exchange_id: self.exchange_id.project(arena)?,
            sequence: self.sequence.project(arena)?,
            lagged: self.lagged.project(arena)?,
        })
    }
}
pub fn error_category_number(value: &transmog_app::ErrorCategory) -> u32 {
    match value {
        transmog_app::ErrorCategory::PasswordRequired => 1,
        transmog_app::ErrorCategory::InvalidPassword => 2,
        transmog_app::ErrorCategory::InvalidInput => 3,
        transmog_app::ErrorCategory::Unsupported => 4,
        transmog_app::ErrorCategory::Canceled => 5,
        transmog_app::ErrorCategory::Conflict => 6,
        transmog_app::ErrorCategory::Limit => 7,
        transmog_app::ErrorCategory::Unavailable => 8,
        transmog_app::ErrorCategory::Retryable => 9,
        transmog_app::ErrorCategory::Internal => 10,
    }
}
impl Project for transmog_app::ErrorCategory {
    type View = u32;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(error_category_number(self))
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ViewAppError {
    pub category: u32,
    pub message: Text,
    pub retryable: u32,
}
impl Project for transmog_app::AppError {
    type View = ViewAppError;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let _ = &arena;
        Ok(ViewAppError {
            category: self.category.project(arena)?,
            message: self.message.project(arena)?,
            retryable: self.retryable.project(arena)?,
        })
    }
}
impl Model for transmog_app::AppStatus {
    const KIND: u32 = 1;
}
impl Model for transmog_app::ProductState {
    const KIND: u32 = 2;
}
impl Model for transmog_app::ProxyStartRequest {
    const KIND: u32 = 3;
}
impl Model for transmog_app::SessionQueryInput {
    const KIND: u32 = 4;
}
impl Model for transmog_app::SessionPage {
    const KIND: u32 = 5;
}
impl Model for transmog_app::TrafficView {
    const KIND: u32 = 6;
}
impl Model for transmog_app::SessionSummary {
    const KIND: u32 = 7;
}
impl Model for transmog_app::SessionDetail {
    const KIND: u32 = 8;
}
impl Model for transmog_app::SessionHint {
    const KIND: u32 = 9;
}
impl Model for transmog_app::AppError {
    const KIND: u32 = 10;
}
impl Model for Vec<transmog_app::SessionSummary> {
    const KIND: u32 = 11;
}
pub fn layout(index: usize) -> usize {
    match index {
        0 => std::mem::size_of::<u32>(),
        1 => std::mem::align_of::<u32>(),
        2 => std::mem::size_of::<Optional<Text>>(),
        3 => std::mem::align_of::<Optional<Text>>(),
        4 => std::mem::size_of::<ViewAppStatus>(),
        5 => std::mem::align_of::<ViewAppStatus>(),
        6 => std::mem::size_of::<ViewAllowedHttpProtocols>(),
        7 => std::mem::align_of::<ViewAllowedHttpProtocols>(),
        8 => std::mem::size_of::<u32>(),
        9 => std::mem::align_of::<u32>(),
        10 => std::mem::size_of::<u32>(),
        11 => std::mem::align_of::<u32>(),
        12 => std::mem::size_of::<u32>(),
        13 => std::mem::align_of::<u32>(),
        14 => std::mem::size_of::<ViewNetworkProfileSettings>(),
        15 => std::mem::align_of::<ViewNetworkProfileSettings>(),
        16 => std::mem::size_of::<ViewNetworkProfileSnapshot>(),
        17 => std::mem::align_of::<ViewNetworkProfileSnapshot>(),
        18 => std::mem::size_of::<Slice<ViewNetworkProfileSnapshot>>(),
        19 => std::mem::align_of::<Slice<ViewNetworkProfileSnapshot>>(),
        20 => std::mem::size_of::<ViewNetworkProfiles>(),
        21 => std::mem::align_of::<ViewNetworkProfiles>(),
        22 => std::mem::size_of::<Optional<u16>>(),
        23 => std::mem::align_of::<Optional<u16>>(),
        24 => std::mem::size_of::<u32>(),
        25 => std::mem::align_of::<u32>(),
        26 => std::mem::size_of::<Slice<TextEntry>>(),
        27 => std::mem::align_of::<Slice<TextEntry>>(),
        28 => std::mem::size_of::<ViewClientCertificateSelector>(),
        29 => std::mem::align_of::<ViewClientCertificateSelector>(),
        30 => std::mem::size_of::<ViewClientCertificateAction>(),
        31 => std::mem::align_of::<ViewClientCertificateAction>(),
        32 => std::mem::size_of::<ViewClientCertificateRule>(),
        33 => std::mem::align_of::<ViewClientCertificateRule>(),
        34 => std::mem::size_of::<Slice<ViewClientCertificateRule>>(),
        35 => std::mem::align_of::<Slice<ViewClientCertificateRule>>(),
        36 => std::mem::size_of::<ViewClientCertificatePolicy>(),
        37 => std::mem::align_of::<ViewClientCertificatePolicy>(),
        38 => std::mem::size_of::<u32>(),
        39 => std::mem::align_of::<u32>(),
        40 => std::mem::size_of::<ViewProductPreferences>(),
        41 => std::mem::align_of::<ViewProductPreferences>(),
        42 => std::mem::size_of::<ViewBufferLimit>(),
        43 => std::mem::align_of::<ViewBufferLimit>(),
        44 => std::mem::size_of::<Optional<usize>>(),
        45 => std::mem::align_of::<Optional<usize>>(),
        46 => std::mem::size_of::<Optional<u64>>(),
        47 => std::mem::align_of::<Optional<u64>>(),
        48 => std::mem::size_of::<ViewPrivacySettings>(),
        49 => std::mem::align_of::<ViewPrivacySettings>(),
        50 => std::mem::size_of::<Optional<i32>>(),
        51 => std::mem::align_of::<Optional<i32>>(),
        52 => std::mem::size_of::<ViewWindowState>(),
        53 => std::mem::align_of::<ViewWindowState>(),
        54 => std::mem::size_of::<u32>(),
        55 => std::mem::align_of::<u32>(),
        56 => std::mem::size_of::<ViewRecentArtifact>(),
        57 => std::mem::align_of::<ViewRecentArtifact>(),
        58 => std::mem::size_of::<Slice<ViewRecentArtifact>>(),
        59 => std::mem::align_of::<Slice<ViewRecentArtifact>>(),
        60 => std::mem::size_of::<u32>(),
        61 => std::mem::align_of::<u32>(),
        62 => std::mem::size_of::<u32>(),
        63 => std::mem::align_of::<u32>(),
        64 => std::mem::size_of::<u32>(),
        65 => std::mem::align_of::<u32>(),
        66 => std::mem::size_of::<ViewColumnPreference>(),
        67 => std::mem::align_of::<ViewColumnPreference>(),
        68 => std::mem::size_of::<Slice<ViewColumnPreference>>(),
        69 => std::mem::align_of::<Slice<ViewColumnPreference>>(),
        70 => std::mem::size_of::<Slice<Text>>(),
        71 => std::mem::align_of::<Slice<Text>>(),
        72 => std::mem::size_of::<ViewWorkspacePreferences>(),
        73 => std::mem::align_of::<ViewWorkspacePreferences>(),
        74 => std::mem::size_of::<ViewProductState>(),
        75 => std::mem::align_of::<ViewProductState>(),
        76 => std::mem::size_of::<u32>(),
        77 => std::mem::align_of::<u32>(),
        78 => std::mem::size_of::<ViewProxyStartRequest>(),
        79 => std::mem::align_of::<ViewProxyStartRequest>(),
        80 => std::mem::size_of::<ViewIncidentWindow>(),
        81 => std::mem::align_of::<ViewIncidentWindow>(),
        82 => std::mem::size_of::<Optional<ViewIncidentWindow>>(),
        83 => std::mem::align_of::<Optional<ViewIncidentWindow>>(),
        84 => std::mem::size_of::<Optional<u32>>(),
        85 => std::mem::align_of::<Optional<u32>>(),
        86 => std::mem::size_of::<u32>(),
        87 => std::mem::align_of::<u32>(),
        88 => std::mem::size_of::<ViewSessionColumnFilter>(),
        89 => std::mem::align_of::<ViewSessionColumnFilter>(),
        90 => std::mem::size_of::<Slice<ViewSessionColumnFilter>>(),
        91 => std::mem::align_of::<Slice<ViewSessionColumnFilter>>(),
        92 => std::mem::size_of::<u32>(),
        93 => std::mem::align_of::<u32>(),
        94 => std::mem::size_of::<ViewTrafficDimension>(),
        95 => std::mem::align_of::<ViewTrafficDimension>(),
        96 => std::mem::size_of::<ViewTrafficFilterValue>(),
        97 => std::mem::align_of::<ViewTrafficFilterValue>(),
        98 => std::mem::size_of::<Slice<ViewTrafficFilterValue>>(),
        99 => std::mem::align_of::<Slice<ViewTrafficFilterValue>>(),
        100 => std::mem::size_of::<ViewTrafficFilterGroup>(),
        101 => std::mem::align_of::<ViewTrafficFilterGroup>(),
        102 => std::mem::size_of::<Slice<ViewTrafficFilterGroup>>(),
        103 => std::mem::align_of::<Slice<ViewTrafficFilterGroup>>(),
        104 => std::mem::size_of::<ViewClassificationFilter>(),
        105 => std::mem::align_of::<ViewClassificationFilter>(),
        106 => std::mem::size_of::<Slice<ViewClassificationFilter>>(),
        107 => std::mem::align_of::<Slice<ViewClassificationFilter>>(),
        108 => std::mem::size_of::<u32>(),
        109 => std::mem::align_of::<u32>(),
        110 => std::mem::size_of::<ViewSessionSort>(),
        111 => std::mem::align_of::<ViewSessionSort>(),
        112 => std::mem::size_of::<Optional<ViewSessionSort>>(),
        113 => std::mem::align_of::<Optional<ViewSessionSort>>(),
        114 => std::mem::size_of::<ViewSessionQueryInput>(),
        115 => std::mem::align_of::<ViewSessionQueryInput>(),
        116 => std::mem::size_of::<Optional<u32>>(),
        117 => std::mem::align_of::<Optional<u32>>(),
        118 => std::mem::size_of::<ViewClientIdentityView>(),
        119 => std::mem::align_of::<ViewClientIdentityView>(),
        120 => std::mem::size_of::<u32>(),
        121 => std::mem::align_of::<u32>(),
        122 => std::mem::size_of::<Optional<u32>>(),
        123 => std::mem::align_of::<Optional<u32>>(),
        124 => std::mem::size_of::<ViewStreamLifetime>(),
        125 => std::mem::align_of::<ViewStreamLifetime>(),
        126 => std::mem::size_of::<Optional<ViewStreamLifetime>>(),
        127 => std::mem::align_of::<Optional<ViewStreamLifetime>>(),
        128 => std::mem::size_of::<ViewLocalResponseSnapshot>(),
        129 => std::mem::align_of::<ViewLocalResponseSnapshot>(),
        130 => std::mem::size_of::<Optional<ViewLocalResponseSnapshot>>(),
        131 => std::mem::align_of::<Optional<ViewLocalResponseSnapshot>>(),
        132 => std::mem::size_of::<ViewAutoResponseMatchView>(),
        133 => std::mem::align_of::<ViewAutoResponseMatchView>(),
        134 => std::mem::size_of::<Optional<ViewAutoResponseMatchView>>(),
        135 => std::mem::align_of::<Optional<ViewAutoResponseMatchView>>(),
        136 => std::mem::size_of::<ViewSessionSummary>(),
        137 => std::mem::align_of::<ViewSessionSummary>(),
        138 => std::mem::size_of::<Slice<ViewSessionSummary>>(),
        139 => std::mem::align_of::<Slice<ViewSessionSummary>>(),
        140 => std::mem::size_of::<ViewSessionPage>(),
        141 => std::mem::align_of::<ViewSessionPage>(),
        142 => std::mem::size_of::<ViewTrafficView>(),
        143 => std::mem::align_of::<ViewTrafficView>(),
        144 => std::mem::size_of::<u32>(),
        145 => std::mem::align_of::<u32>(),
        146 => std::mem::size_of::<ViewStreamState>(),
        147 => std::mem::align_of::<ViewStreamState>(),
        148 => std::mem::size_of::<ViewStreamSummary>(),
        149 => std::mem::align_of::<ViewStreamSummary>(),
        150 => std::mem::size_of::<Optional<ViewStreamSummary>>(),
        151 => std::mem::align_of::<Optional<ViewStreamSummary>>(),
        152 => std::mem::size_of::<ViewComposerOrigin>(),
        153 => std::mem::align_of::<ViewComposerOrigin>(),
        154 => std::mem::size_of::<Optional<ViewComposerOrigin>>(),
        155 => std::mem::align_of::<Optional<ViewComposerOrigin>>(),
        156 => std::mem::size_of::<u32>(),
        157 => std::mem::align_of::<u32>(),
        158 => std::mem::size_of::<ViewTimingPoint>(),
        159 => std::mem::align_of::<ViewTimingPoint>(),
        160 => std::mem::size_of::<Slice<ViewTimingPoint>>(),
        161 => std::mem::align_of::<Slice<ViewTimingPoint>>(),
        162 => std::mem::size_of::<u32>(),
        163 => std::mem::align_of::<u32>(),
        164 => std::mem::size_of::<ViewConnectionTiming>(),
        165 => std::mem::align_of::<ViewConnectionTiming>(),
        166 => std::mem::size_of::<Slice<ViewConnectionTiming>>(),
        167 => std::mem::align_of::<Slice<ViewConnectionTiming>>(),
        168 => std::mem::size_of::<Slice<u8>>(),
        169 => std::mem::align_of::<Slice<u8>>(),
        170 => std::mem::size_of::<ViewAlpsEvidence>(),
        171 => std::mem::align_of::<ViewAlpsEvidence>(),
        172 => std::mem::size_of::<Optional<ViewAlpsEvidence>>(),
        173 => std::mem::align_of::<Optional<ViewAlpsEvidence>>(),
        174 => std::mem::size_of::<Optional<ViewNetworkProfileSnapshot>>(),
        175 => std::mem::align_of::<Optional<ViewNetworkProfileSnapshot>>(),
        176 => std::mem::size_of::<ViewPublicCertificate>(),
        177 => std::mem::align_of::<ViewPublicCertificate>(),
        178 => std::mem::size_of::<Slice<ViewPublicCertificate>>(),
        179 => std::mem::align_of::<Slice<ViewPublicCertificate>>(),
        180 => std::mem::size_of::<ViewVerificationResult>(),
        181 => std::mem::align_of::<ViewVerificationResult>(),
        182 => std::mem::size_of::<Optional<ViewVerificationResult>>(),
        183 => std::mem::align_of::<Optional<ViewVerificationResult>>(),
        184 => std::mem::size_of::<ViewTlsEvidence>(),
        185 => std::mem::align_of::<ViewTlsEvidence>(),
        186 => std::mem::size_of::<Optional<ViewTlsEvidence>>(),
        187 => std::mem::align_of::<Optional<ViewTlsEvidence>>(),
        188 => std::mem::size_of::<ViewTcpObservation>(),
        189 => std::mem::align_of::<ViewTcpObservation>(),
        190 => std::mem::size_of::<Optional<ViewTcpObservation>>(),
        191 => std::mem::align_of::<Optional<ViewTcpObservation>>(),
        192 => std::mem::size_of::<Optional<i64>>(),
        193 => std::mem::align_of::<Optional<i64>>(),
        194 => std::mem::size_of::<ViewSocketIoObservation>(),
        195 => std::mem::align_of::<ViewSocketIoObservation>(),
        196 => std::mem::size_of::<Optional<ViewSocketIoObservation>>(),
        197 => std::mem::align_of::<Optional<ViewSocketIoObservation>>(),
        198 => std::mem::size_of::<ViewQuicObservation>(),
        199 => std::mem::align_of::<ViewQuicObservation>(),
        200 => std::mem::size_of::<Optional<ViewQuicObservation>>(),
        201 => std::mem::align_of::<Optional<ViewQuicObservation>>(),
        202 => std::mem::size_of::<ViewTransportObservation>(),
        203 => std::mem::align_of::<ViewTransportObservation>(),
        204 => std::mem::size_of::<Slice<ViewTransportObservation>>(),
        205 => std::mem::align_of::<Slice<ViewTransportObservation>>(),
        206 => std::mem::size_of::<ViewProtocolObservation>(),
        207 => std::mem::align_of::<ViewProtocolObservation>(),
        208 => std::mem::size_of::<Slice<ViewProtocolObservation>>(),
        209 => std::mem::align_of::<Slice<ViewProtocolObservation>>(),
        210 => std::mem::size_of::<ViewWorkTiming>(),
        211 => std::mem::align_of::<ViewWorkTiming>(),
        212 => std::mem::size_of::<Slice<ViewWorkTiming>>(),
        213 => std::mem::align_of::<Slice<ViewWorkTiming>>(),
        214 => std::mem::size_of::<ViewPerformanceEvidence>(),
        215 => std::mem::align_of::<ViewPerformanceEvidence>(),
        216 => std::mem::size_of::<ViewHeaderSummary>(),
        217 => std::mem::align_of::<ViewHeaderSummary>(),
        218 => std::mem::size_of::<ViewHeaderView>(),
        219 => std::mem::align_of::<ViewHeaderView>(),
        220 => std::mem::size_of::<Slice<ViewHeaderView>>(),
        221 => std::mem::align_of::<Slice<ViewHeaderView>>(),
        222 => std::mem::size_of::<ViewHeadView>(),
        223 => std::mem::align_of::<ViewHeadView>(),
        224 => std::mem::size_of::<Slice<ViewHeadView>>(),
        225 => std::mem::align_of::<Slice<ViewHeadView>>(),
        226 => std::mem::size_of::<ViewBodyView>(),
        227 => std::mem::align_of::<ViewBodyView>(),
        228 => std::mem::size_of::<Slice<ViewBodyView>>(),
        229 => std::mem::align_of::<Slice<ViewBodyView>>(),
        230 => std::mem::size_of::<u32>(),
        231 => std::mem::align_of::<u32>(),
        232 => std::mem::size_of::<ViewStoredBodyMetadata>(),
        233 => std::mem::align_of::<ViewStoredBodyMetadata>(),
        234 => std::mem::size_of::<Slice<ViewStoredBodyMetadata>>(),
        235 => std::mem::align_of::<Slice<ViewStoredBodyMetadata>>(),
        236 => std::mem::size_of::<ViewHeaderChanges>(),
        237 => std::mem::align_of::<ViewHeaderChanges>(),
        238 => std::mem::size_of::<ViewHeaderEffectView>(),
        239 => std::mem::align_of::<ViewHeaderEffectView>(),
        240 => std::mem::size_of::<Slice<ViewHeaderEffectView>>(),
        241 => std::mem::align_of::<Slice<ViewHeaderEffectView>>(),
        242 => std::mem::size_of::<ViewBodyEditReport>(),
        243 => std::mem::align_of::<ViewBodyEditReport>(),
        244 => std::mem::size_of::<ViewBodyEditView>(),
        245 => std::mem::align_of::<ViewBodyEditView>(),
        246 => std::mem::size_of::<Slice<ViewBodyEditView>>(),
        247 => std::mem::align_of::<Slice<ViewBodyEditView>>(),
        248 => std::mem::size_of::<ViewSessionDetail>(),
        249 => std::mem::align_of::<ViewSessionDetail>(),
        250 => std::mem::size_of::<ViewSessionHint>(),
        251 => std::mem::align_of::<ViewSessionHint>(),
        252 => std::mem::size_of::<u32>(),
        253 => std::mem::align_of::<u32>(),
        254 => std::mem::size_of::<ViewAppError>(),
        255 => std::mem::align_of::<ViewAppError>(),
        _ => 0,
    }
}
