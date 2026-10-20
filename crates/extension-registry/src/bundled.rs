use super::{ExtensionEntry, State};
use std::collections::BTreeMap;
use transmog_extensions::{CatalogEntry, Manifest, Origin, sha256};

/// Trusted app resources with a separately generated metadata sidecar.
#[derive(Clone, Copy)]
pub struct BundledExtension {
    /// Bounded generated manifest bytes; discovery never opens the archive.
    pub manifest: &'static [u8],
    /// Returns immutable ordinary tmext bytes only upon explicit activation.
    pub archive: fn() -> &'static [u8],
}

pub(super) fn merge(
    mut state: State,
    resources: &[BundledExtension],
) -> (State, BTreeMap<String, BundledExtension>) {
    let catalog_broken = state.warning.is_some();
    let mut bundled = BTreeMap::new();
    for resource in resources {
        let Ok(manifest) = Manifest::parse(resource.manifest, false) else {
            state.warning.get_or_insert_with(|| {
                "A bundled extension declaration is unavailable in this app version.".into()
            });
            continue;
        };
        let id = manifest.id.clone();
        if bundled.contains_key(&id) {
            state.warning.get_or_insert_with(|| {
                "The app declares a bundled extension ID more than once.".into()
            });
            continue;
        }
        let prior = state
            .entries
            .iter()
            .position(|entry| entry.catalog.manifest.id == id);
        if let Some(index) = prior
            && state.entries[index].catalog.origin != Origin::Bundled
        {
            // Preserve user-owned source/packages and their management controls.
            // Do not silently replace their code or claim app provenance.
            state.entries[index].catalog.enabled = false;
            state.warning.get_or_insert_with(|| format!("A user extension conflicts with the bundled {} ID. Remove that installation and restart to use the bundled copy.",manifest.name));
            continue;
        }
        if prior.is_none() && state.entries.len() >= super::MAX_INSTALLED {
            state.warning.get_or_insert_with(|| "Remove an installed extension and restart to make room for the app's bundled extensions.".into());
            continue;
        }
        let enabled =
            !catalog_broken && prior.is_none_or(|index| state.entries[index].catalog.enabled);
        let digest = sha256(&serde_json::to_vec(&manifest).expect("validated manifests serialize"));
        let entry = ExtensionEntry {
            catalog: CatalogEntry {
                manifest,
                digest,
                enabled,
                origin: Origin::Bundled,
            },
            directory: None,
            hot_reload: false,
        };
        if let Some(index) = prior {
            state.entries[index] = entry;
        } else {
            state.entries.push(entry);
        }
        bundled.insert(id, *resource);
    }
    // Cached metadata cannot make an absent app-owned resource executable.
    for entry in &mut state.entries {
        if entry.catalog.origin == Origin::Bundled
            && !bundled.contains_key(&entry.catalog.manifest.id)
        {
            entry.catalog.enabled = false;
        }
    }
    (state, bundled)
}
