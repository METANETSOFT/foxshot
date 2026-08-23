//! FoxShot Core — platform-agnostic foundation.
//!
//! This crate contains the shared vocabulary of FoxShot: errors, geometry,
//! captured frames, and the platform trait set. It deliberately contains no
//! operating-system-specific code — every platform capability lives behind a
//! trait in [`platform`], and nothing in Core branches on `cfg(target_os)`.

pub mod annotation;
pub mod error;
pub mod frame;
pub mod geometry;
pub mod module;
pub mod platform;
pub mod selection;
pub mod testing;
pub mod update;
pub mod upload;

pub use annotation::{AnnotationDocument, Finding, Ink, Mark, MarkId, MarkKind};
pub use error::{Error, Result};
pub use frame::Frame;
pub use geometry::{Point, Rect, Scale, Size};
pub use module::{Component, ModuleInfo, ModuleRegistry, ModuleState, Version};
pub use selection::{Handle, SelectionPhase, SelectionState};
pub use platform::{
    ButtonSide, ChromeStyle, ClipboardService, Display, Fetch, HotkeyService,
    NotificationService, Paths, Permission, PermissionService, PermissionState, Platform,
    ScreenCapture, ScreenService, WindowChrome,
};
pub use testing::NullPlatform;
pub use update::{ManifestEntry, UpdateChecker, UpdateManifest, UpdateReport, UpdateStatus};
pub use upload::{
    Credentials, DrainReport, FreeHostTarget, PendingUpload, S3Target, UploadQueue, UploadTarget,
};

/// Crate version, taken from the package manifest at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
