//! Immutable build and bundled-source identities for support reports.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Identity of the compiled application or embedding host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildIdentity {
    /// Source commit, absent when Git metadata was unavailable during the build.
    pub source_commit: Option<String>,
    /// Whether the source checkout was modified; absent without a fresh snapshot from the portable runner.
    pub source_dirty: Option<bool>,
    /// Cargo target triple for the compiled code.
    pub target: String,
    /// Cargo build profile.
    pub profile: String,
    /// Rust compiler version used for the build.
    pub rustc: String,
}

/// One version from the locked Rust graph or bundled browser assets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildDependency {
    /// Canonical package name.
    pub name: String,
    /// Readable support label.
    pub label: String,
    /// Exact package version.
    pub version: String,
}

/// Identity of source code or policy data bundled with this build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledSource {
    /// Readable source or data-set name.
    pub name: String,
    /// Upstream source commit, when available.
    pub revision: Option<String>,
    /// Bundled release or data version, when available.
    pub version: Option<String>,
    /// Local patch SHA-256, when applicable.
    pub patch_sha256: Option<String>,
}

/// Build metadata supplied once by a native embedding host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostBuildInfo {
    /// Identity of the host executable.
    pub identity: BuildIdentity,
    /// Host-specific packages, without linking them into the application facade.
    pub dependencies: Vec<BuildDependency>,
}

/// Complete immutable build information included in diagnostics and support bundles.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// Host executable identity, or application-library identity without a host.
    pub identity: BuildIdentity,
    /// Application, worker-toolchain and optional host dependency versions.
    pub dependencies: Vec<BuildDependency>,
    /// Bundled baselines; effective runtime policy remains separate.
    pub bundled_sources: Vec<BundledSource>,
}

impl BuildInfo {
    pub(crate) fn embedded(host: Option<HostBuildInfo>) -> Self {
        static APPLICATION: LazyLock<BuildInfo> = LazyLock::new(|| {
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/build-info.json")))
                .expect("generated application build metadata")
        });
        let mut info = APPLICATION.clone();
        if let Some(host) = host {
            info.identity = host.identity;
            info.dependencies.extend(host.dependencies);
        }
        info
    }
}
