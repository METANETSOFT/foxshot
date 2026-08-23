//! Update manifests and the pure comparison logic that turns
//! "what is installed" plus "what exists upstream" into an update report.
//!
//! Core does **no networking**: the caller hands [`UpdateManifest::from_json`]
//! a JSON string that a platform adapter retrieved through its
//! [`crate::platform::Fetch`] trait. Everything here is deterministic data
//! in, report out.

use crate::error::{Error, Result};
use crate::module::{Component, ModuleRegistry, Version};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The only manifest schema this build understands.
pub const MANIFEST_SCHEMA: u32 = 1;

/// One entry of the update manifest: the latest published version of a
/// component, plus everything needed to fetch and verify it.
///
/// `download: null` (or a missing `download`) parses to `None` and means
/// the version is **reportable but not installable** — the UI may announce
/// it, but there is no artifact to pull.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestEntry {
    /// The latest published version.
    pub version: Version,
    /// Minimum Core version this release can run against, if constrained.
    #[serde(default)]
    pub min_core: Option<Version>,
    /// Download URL of the artifact; `None` means "not installable".
    #[serde(default)]
    pub download: Option<String>,
    /// SHA-256 of the artifact, hex-encoded.
    #[serde(default)]
    pub sha256: Option<String>,
    /// URL of release notes.
    #[serde(default)]
    pub changelog: Option<String>,
}

/// The parsed `updates.json` published by the FoxShot project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UpdateManifest {
    /// Manifest schema number; only [`MANIFEST_SCHEMA`] is accepted.
    pub schema: u32,
    /// The Core entry.
    pub core: ManifestEntry,
    /// Adapter entries by adapter name.
    #[serde(default)]
    pub adapters: BTreeMap<String, ManifestEntry>,
    /// Module entries by module name.
    #[serde(default)]
    pub modules: BTreeMap<String, ManifestEntry>,
}

impl UpdateManifest {
    /// Parses a manifest from its JSON text.
    ///
    /// Fails with [`Error::Manifest`] on invalid JSON, on entries that do
    /// not fit the schema, and on a `schema` number this build does not
    /// understand — an unknown schema must be rejected, not guessed at.
    pub fn from_json(json: &str) -> Result<Self> {
        let manifest: UpdateManifest = serde_json::from_str(json)
            .map_err(|e| Error::Manifest { message: format!("invalid update manifest: {e}") })?;
        if manifest.schema != MANIFEST_SCHEMA {
            return Err(Error::Manifest {
                message: format!(
                    "unsupported manifest schema {} (this build understands {MANIFEST_SCHEMA})",
                    manifest.schema
                ),
            });
        }
        Ok(manifest)
    }
}

/// The update situation of one component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// The installed version is equal to or newer than the published one.
    UpToDate,
    /// A newer version is published.
    Available {
        /// Currently installed version.
        from: Version,
        /// Published version.
        to: Version,
        /// True when the manifest names a downloadable artifact.
        installable: bool,
    },
    /// A newer version is published but it needs a newer Core than the one
    /// running — Core must be updated first.
    BlockedByCore {
        /// Core version the release requires.
        needs: Version,
        /// Core version currently running.
        have: Version,
    },
}

/// The outcome of comparing a registry against a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateReport {
    /// Update status of Core itself.
    pub core: UpdateStatus,
    /// Update status of every adapter and module named by the manifest.
    pub per_component: Vec<(Component, UpdateStatus)>,
}

impl UpdateReport {
    /// True when anything — Core, an adapter, or a module — has a newer
    /// published version, whether installable or blocked.
    pub fn has_any_update(&self) -> bool {
        !matches!(self.core, UpdateStatus::UpToDate)
            || self.per_component.iter().any(|(_, s)| !matches!(s, UpdateStatus::UpToDate))
    }

    /// True only when Core itself has an update available. A Core update
    /// replaces the running binary, so the app must restart to apply it;
    /// adapter and module updates load without a restart.
    pub fn requires_restart(&self) -> bool {
        matches!(self.core, UpdateStatus::Available { .. })
    }
}

/// Compares what is installed with what is published.
#[derive(Debug)]
pub struct UpdateChecker;

impl UpdateChecker {
    /// Builds the update report for `registry` against `manifest`.
    ///
    /// Rules, per component: a published version equal to or older than the
    /// installed one is [`UpdateStatus::UpToDate`]; a newer one whose
    /// `min_core` exceeds the running Core is [`UpdateStatus::BlockedByCore`];
    /// otherwise it is [`UpdateStatus::Available`], `installable` exactly
    /// when the entry names a download. A component that is not installed at
    /// all counts as version `0.0.0`, so any published release is newer.
    pub fn compare(registry: &ModuleRegistry, manifest: &UpdateManifest) -> UpdateReport {
        let have_core = registry.core_version();
        let core = Self::status_for(Some(have_core), &manifest.core, have_core);

        let mut per_component = Vec::new();
        for (name, entry) in &manifest.adapters {
            let component = Component::Adapter(name.clone());
            per_component.push((component.clone(), Self::status_for(
                Self::installed_version(registry, &component), entry, have_core,
            )));
        }
        for (name, entry) in &manifest.modules {
            let component = Component::Module(name.clone());
            per_component.push((component.clone(), Self::status_for(
                Self::installed_version(registry, &component), entry, have_core,
            )));
        }

        UpdateReport { core, per_component }
    }

    /// The installed version of a component, or `None` when absent.
    fn installed_version(registry: &ModuleRegistry, component: &Component) -> Option<Version> {
        match registry.state(component) {
            crate::module::ModuleState::Installed(version) => Some(version),
            _ => None,
        }
    }

    /// The status of one component given its installed version (if any).
    fn status_for(installed: Option<Version>, entry: &ManifestEntry, have_core: Version) -> UpdateStatus {
        let from = installed.unwrap_or(Version::new(0, 0, 0));
        if entry.version <= from {
            return UpdateStatus::UpToDate;
        }
        if let Some(needs) = entry.min_core {
            if needs > have_core {
                return UpdateStatus::BlockedByCore { needs, have: have_core };
            }
        }
        UpdateStatus::Available { from, to: entry.version, installable: entry.download.is_some() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ModuleState;

    /// The real manifest, fetched from the repository once and committed as
    /// a fixture so the test runs offline and against stable bytes.
    const REAL_MANIFEST: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/updates.json"));

    /// A registry with Core, all three adapters and all five modules at 0.1.0.
    fn registry_all_0_1_0() -> ModuleRegistry {
        let mut registry = ModuleRegistry::new().with_installed(
            Component::Core,
            Version::new(0, 1, 0),
        );
        for adapter in ["linux", "macos", "windows"] {
            registry.bump(&Component::Adapter(adapter.into()), Version::new(0, 1, 0));
        }
        for module in ["editor", "upload", "video", "ocr", "qr"] {
            registry.bump(&Component::Module(module.into()), Version::new(0, 1, 0));
        }
        registry
    }

    fn status_of<'a>(report: &'a UpdateReport, component: &Component) -> &'a UpdateStatus {
        &report
            .per_component
            .iter()
            .find(|(c, _)| c == component)
            .unwrap_or_else(|| panic!("no status for {component}"))
            .1
    }

    #[test]
    fn real_manifest_parses() {
        let manifest = UpdateManifest::from_json(REAL_MANIFEST).unwrap();
        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.core.version, Version::new(0, 1, 0));
        assert_eq!(manifest.adapters.len(), 3);
        assert_eq!(manifest.modules.len(), 5);
        for name in ["linux", "macos", "windows"] {
            assert!(manifest.adapters.contains_key(name), "adapter {name} missing");
        }
        for name in ["editor", "upload", "video", "ocr", "qr"] {
            assert!(manifest.modules.contains_key(name), "module {name} missing");
        }
        // `download: null` parses to None and min_core round-trips.
        assert_eq!(manifest.core.download, None);
        assert_eq!(manifest.adapters["linux"].min_core, Some(Version::new(0, 1, 0)));
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let json = REAL_MANIFEST.replace("\"schema\": 1", "\"schema\": 99");
        assert_ne!(json, REAL_MANIFEST, "fixture must contain the schema field");
        match UpdateManifest::from_json(&json) {
            Err(Error::Manifest { message }) => assert!(message.contains("99")),
            other => panic!("expected Error::Manifest, got {other:?}"),
        }
        assert!(UpdateManifest::from_json("{not json").is_err());
    }

    #[test]
    fn real_manifest_against_fresh_registry_has_no_updates() {
        let manifest = UpdateManifest::from_json(REAL_MANIFEST).unwrap();
        let report = UpdateChecker::compare(&registry_all_0_1_0(), &manifest);
        assert!(!report.has_any_update());
        assert!(!report.requires_restart());
        assert_eq!(report.core, UpdateStatus::UpToDate);
        assert_eq!(report.per_component.len(), 8);
    }

    #[test]
    fn bumped_core_requires_restart() {
        let mut manifest = UpdateManifest::from_json(REAL_MANIFEST).unwrap();
        manifest.core.version = Version::new(0, 2, 0);
        manifest.core.download = Some("https://example.invalid/foxshot-0.2.0.tar.gz".into());

        let report = UpdateChecker::compare(&registry_all_0_1_0(), &manifest);
        assert_eq!(
            report.core,
            UpdateStatus::Available {
                from: Version::new(0, 1, 0),
                to: Version::new(0, 2, 0),
                installable: true,
            }
        );
        assert!(report.requires_restart());
        assert!(report.has_any_update());
    }

    #[test]
    fn module_min_core_above_core_is_blocked() {
        let mut manifest = UpdateManifest::from_json(REAL_MANIFEST).unwrap();
        let editor = manifest.modules.get_mut("editor").unwrap();
        editor.version = Version::new(0, 2, 0);
        editor.min_core = Some(Version::new(0, 5, 0));
        editor.download = Some("https://example.invalid/editor-0.2.0.tar.gz".into());

        let report = UpdateChecker::compare(&registry_all_0_1_0(), &manifest);
        assert_eq!(
            status_of(&report, &Component::Module("editor".into())),
            &UpdateStatus::BlockedByCore {
                needs: Version::new(0, 5, 0),
                have: Version::new(0, 1, 0),
            }
        );
        // A blocked module is still a published update, but not a restart.
        assert!(report.has_any_update());
        assert!(!report.requires_restart());
    }

    #[test]
    fn newer_module_without_download_is_not_installable() {
        let mut manifest = UpdateManifest::from_json(REAL_MANIFEST).unwrap();
        manifest.modules.get_mut("video").unwrap().version = Version::new(0, 2, 0);
        // `download` stays null: reportable but not installable.
        assert_eq!(manifest.modules["video"].download, None);

        let report = UpdateChecker::compare(&registry_all_0_1_0(), &manifest);
        assert_eq!(
            status_of(&report, &Component::Module("video".into())),
            &UpdateStatus::Available {
                from: Version::new(0, 1, 0),
                to: Version::new(0, 2, 0),
                installable: false,
            }
        );
        assert!(!report.requires_restart());
    }

    #[test]
    fn not_installed_component_counts_as_zero() {
        let manifest = UpdateManifest::from_json(REAL_MANIFEST).unwrap();
        let registry = ModuleRegistry::new().with_installed(
            Component::Core,
            Version::new(0, 1, 0),
        );
        let report = UpdateChecker::compare(&registry, &manifest);
        assert_eq!(
            status_of(&report, &Component::Adapter("linux".into())),
            &UpdateStatus::Available {
                from: Version::new(0, 0, 0),
                to: Version::new(0, 1, 0),
                installable: false,
            }
        );
        assert_eq!(
            registry.state(&Component::Adapter("linux".into())),
            ModuleState::NotInstalled
        );
    }
}
