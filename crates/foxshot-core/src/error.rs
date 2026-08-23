//! The single error type shared by all of FoxShot Core.

use crate::geometry::Rect;
use core::fmt;

/// Everything that can go wrong inside Core, expressed without any
/// platform-specific error type leaking across the trait boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The user (or the OS) denied a required permission.
    PermissionDenied {
        /// Human-readable name of the permission that was denied.
        permission: String,
    },
    /// No displays were reported by the platform.
    NoDisplays,
    /// A requested rectangle does not fit inside the available bounds.
    RectOutOfBounds {
        /// The rectangle the caller asked for.
        requested: Rect,
        /// The bounds it had to fit inside.
        bounds: Rect,
    },
    /// The platform does not support the requested capability.
    Unsupported {
        /// What was requested but is unsupported.
        what: String,
    },
    /// A transport (network or IPC) operation failed.
    Transport {
        /// Description of the failure.
        message: String,
    },
    /// A manifest (module or update descriptor) failed to parse or validate.
    Manifest {
        /// Description of the failure.
        message: String,
    },
    /// A raw RGBA8 pixel buffer did not match the declared frame size.
    ///
    /// Raised by `Frame` construction only — never reused for manifest or
    /// protocol failures, which have their own variants.
    InvalidPixelBuffer {
        /// Number of bytes the declared size requires (width × height × 4).
        expected: usize,
        /// Number of bytes actually supplied.
        got: usize,
    },
    /// A module requires a Core version different from the one running.
    ModuleIncompatible {
        /// Name of the incompatible module.
        module: String,
        /// Version requirement declared by the module.
        needs: String,
        /// Version actually present.
        have: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::PermissionDenied { permission } => {
                write!(f, "permission denied: {permission}")
            }
            Error::NoDisplays => write!(f, "no displays available"),
            Error::RectOutOfBounds { requested, bounds } => {
                write!(f, "rect {requested:?} is out of bounds {bounds:?}")
            }
            Error::Unsupported { what } => write!(f, "unsupported: {what}"),
            Error::Transport { message } => write!(f, "transport error: {message}"),
            Error::Manifest { message } => write!(f, "manifest error: {message}"),
            Error::InvalidPixelBuffer { expected, got } => {
                write!(f, "invalid pixel buffer: expected {expected} bytes, got {got}")
            }
            Error::ModuleIncompatible { module, needs, have } => {
                write!(f, "module {module} needs core {needs}, have {have}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Convenience result alias used across Core.
pub type Result<T> = core::result::Result<T, Error>;
