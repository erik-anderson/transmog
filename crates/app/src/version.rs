use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Deserialize)]
struct ReleaseVersion {
    version: String,
    channel: String,
}

/// Semantic product version, including the Canary marker for main development.
pub fn application_version() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| {
        let release: ReleaseVersion =
            serde_json::from_str(include_str!("../../../release-version.json"))
                .expect("checked-in release version must be valid");
        if release.channel == "Canary" {
            format!("{} Canary", release.version)
        } else {
            release.version
        }
    });
    &VERSION
}
