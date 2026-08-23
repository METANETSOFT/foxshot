//! Modules, adapters and their versions: the vocabulary of FoxShot's
//! independently updatable parts.
//!
//! FoxShot ships Core, one platform adapter per operating system, and any
//! number of feature modules (editor, upload, video, …). Each has its own
//! version and updates on its own schedule; this module models what is
//! installed and at which version. The update logic in [`crate::update`]
//! builds on top of it.

use crate::error::{Error, Result};
use core::fmt;
use core::str::FromStr;
use serde::Deserialize;
use std::collections::BTreeMap;

/// A semantic version, `major.minor.patch`.
///
/// Ordering is the usual lexicographic one (major, then minor, then patch).
/// Parsing is strict: anything that is not exactly three dot-separated
/// unsigned integers is rejected — a malformed version must never silently
/// become `0.0.0`, because update decisions are made by comparing versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    /// Major version (breaking changes).
    pub major: u32,
    /// Minor version (features).
    pub minor: u32,
    /// Patch version (fixes).
    pub patch: u32,
}

impl Version {
    /// Creates a version from its three components.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let malformed = || Error::Manifest {
            message: format!("malformed version {s:?}: expected major.minor.patch"),
        };
        let mut parts = s.split('.');
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(malformed());
        };
        Ok(Version {
            major: major.parse().map_err(|_| malformed())?,
            minor: minor.parse().map_err(|_| malformed())?,
            patch: patch.parse().map_err(|_| malformed())?,
        })
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The installation state of one component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleState {
    /// The component is installed at the given version.
    Installed(Version),
    /// The component is not installed.
    NotInstalled,
    /// The component is installed but cannot run against this Core.
    Incompatible {
        /// Core version the component requires.
        needs: Version,
        /// Core version actually running.
        have: Version,
    },
}

/// One independently versioned part of FoxShot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Component {
    /// FoxShot Core itself.
    Core,
    /// A platform adapter, by name (`"linux"`, `"macos"`, `"windows"`).
    Adapter(String),
    /// A feature module, by name (`"editor"`, `"upload"`, …).
    Module(String),
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Component::Core => write!(f, "core"),
            Component::Adapter(name) => write!(f, "adapter:{name}"),
            Component::Module(name) => write!(f, "module:{name}"),
        }
    }
}

/// A component together with its installation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    /// Which component.
    pub component: Component,
    /// Its current state.
    pub state: ModuleState,
}

/// The set of components installed on this machine, with their versions.
///
/// Core is always present — the registry itself runs inside Core — so its
/// version is a field of its own, initialised from the crate version and
/// changeable via [`ModuleRegistry::bump`] (or
/// [`ModuleRegistry::with_installed`]) for tests and for the updater.
#[derive(Debug, Clone)]
pub struct ModuleRegistry {
    core: Version,
    installed: BTreeMap<Component, Version>,
}

impl ModuleRegistry {
    /// Creates a registry with only Core installed, at this crate's version.
    pub fn new() -> Self {
        let core = crate::VERSION
            .parse()
            .expect("CARGO_PKG_VERSION is always a valid version");
        Self {
            core,
            installed: BTreeMap::new(),
        }
    }

    /// Builder-style: additionally marks `component` as installed at
    /// `version`. Passing [`Component::Core`] overrides the Core version.
    pub fn with_installed(mut self, component: Component, version: Version) -> Self {
        match component {
            Component::Core => self.core = version,
            other => {
                self.installed.insert(other, version);
            }
        }
        self
    }

    /// The state of a component: [`ModuleState::Installed`] when present,
    /// [`ModuleState::NotInstalled`] otherwise. Compatibility with the
    /// running Core is judged against a manifest, not here — see
    /// [`crate::update::UpdateChecker`].
    pub fn state(&self, component: &Component) -> ModuleState {
        match component {
            Component::Core => ModuleState::Installed(self.core),
            other => match self.installed.get(other) {
                Some(version) => ModuleState::Installed(*version),
                None => ModuleState::NotInstalled,
            },
        }
    }

    /// Everything installed, Core first, the rest ordered by component.
    pub fn installed(&self) -> Vec<ModuleInfo> {
        let mut infos = Vec::with_capacity(self.installed.len() + 1);
        infos.push(ModuleInfo {
            component: Component::Core,
            state: ModuleState::Installed(self.core),
        });
        infos.extend(
            self.installed
                .iter()
                .map(|(component, version)| ModuleInfo {
                    component: component.clone(),
                    state: ModuleState::Installed(*version),
                }),
        );
        infos
    }

    /// Records that a component is now installed at `version` — used after
    /// an update is applied. Unknown components become installed.
    pub fn bump(&mut self, component: &Component, version: Version) {
        match component {
            Component::Core => self.core = version,
            other => {
                self.installed.insert(other.clone(), version);
            }
        }
    }

    /// The running Core version.
    pub fn core_version(&self) -> Version {
        self.core
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parses_orders_and_rejects_malformed() {
        let v: Version = "1.2.3".parse().unwrap();
        assert_eq!(v, Version::new(1, 2, 3));
        assert_eq!(v.to_string(), "1.2.3");

        assert!(Version::new(1, 2, 3) > Version::new(1, 2, 2));
        assert!(Version::new(1, 10, 0) > Version::new(1, 9, 9));
        assert!(Version::new(2, 0, 0) > Version::new(1, 99, 99));
        assert_eq!(Version::new(0, 1, 0), "0.1.0".parse().unwrap());

        // Malformed input is rejected, never silently defaulted to zero.
        assert!("1.2".parse::<Version>().is_err());
        assert!("x.y.z".parse::<Version>().is_err());
        assert!("1.2.3.4".parse::<Version>().is_err());
        assert!("".parse::<Version>().is_err());
        assert!("1.-2.3".parse::<Version>().is_err());
    }

    #[test]
    fn registry_tracks_installed_components() {
        let registry = ModuleRegistry::new()
            .with_installed(Component::Adapter("linux".into()), Version::new(0, 1, 0))
            .with_installed(Component::Module("editor".into()), Version::new(0, 3, 2));

        assert_eq!(
            registry.state(&Component::Module("editor".into())),
            ModuleState::Installed(Version::new(0, 3, 2))
        );
        assert_eq!(
            registry.state(&Component::Module("ocr".into())),
            ModuleState::NotInstalled
        );
        assert_eq!(registry.core_version(), crate::VERSION.parse().unwrap());

        let installed = registry.installed();
        assert_eq!(installed.len(), 3);
        assert_eq!(installed[0].component, Component::Core);
        assert_eq!(installed[0].component.to_string(), "core");
        assert_eq!(installed[1].component.to_string(), "adapter:linux");
        assert_eq!(installed[2].component.to_string(), "module:editor");
    }

    #[test]
    fn bump_updates_versions() {
        let mut registry = ModuleRegistry::new();
        registry.bump(&Component::Core, Version::new(0, 2, 0));
        registry.bump(&Component::Module("video".into()), Version::new(1, 0, 0));
        assert_eq!(registry.core_version(), Version::new(0, 2, 0));
        assert_eq!(
            registry.state(&Component::Module("video".into())),
            ModuleState::Installed(Version::new(1, 0, 0))
        );
    }
}
