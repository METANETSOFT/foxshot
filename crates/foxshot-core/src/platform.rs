//! The platform trait set — every OS capability Core needs, behind objects.
//!
//! Core contains **zero** `cfg(target_os)` branches. A platform adapter
//! (one per operating system) implements exactly the traits in this module
//! and exposes them through a single [`Platform`] value; nothing in Core
//! ever asks which OS it is running on.

use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::geometry::{Rect, Scale};
use std::path::PathBuf;

/// A physical display attached to the system.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// Stable identifier assigned by the platform.
    pub id: u32,
    /// Human-readable display name.
    pub name: String,
    /// Bounds in logical coordinates, global desktop space.
    pub bounds: Rect,
    /// Scale factor of this display.
    pub scale: Scale,
    /// Whether this is the primary display.
    pub is_primary: bool,
}

/// A permission Core may need from the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Recording the screen contents.
    ScreenCapture,
    /// Accessibility access (global input monitoring).
    Accessibility,
    /// Reading and writing the clipboard.
    Clipboard,
}

impl Permission {
    /// A stable, lowercase label used in logs, settings, and error messages.
    pub fn label(&self) -> &'static str {
        match self {
            Permission::ScreenCapture => "screen-capture",
            Permission::Accessibility => "accessibility",
            Permission::Clipboard => "clipboard",
        }
    }
}

/// The current state of a [`Permission`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionState {
    /// The user granted the permission.
    Granted,
    /// The user denied the permission.
    Denied,
    /// The permission has not been asked for yet.
    NotRequested,
}

/// Which side of the titlebar the window buttons live on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonSide {
    /// macOS style: buttons on the left.
    Left,
    /// Windows/Linux style: buttons on the right.
    Right,
}

/// How native window chrome looks on this platform, so the custom-drawn
/// titlebar can match it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowChrome {
    /// Which side the window buttons sit on.
    pub buttons: ButtonSide,
    /// Window corner radius in logical points.
    pub corner_radius: f32,
    /// Titlebar height in logical points.
    pub titlebar_height: f32,
    /// Label of the primary keyboard modifier (e.g. "Cmd", "Ctrl").
    pub modifier_label: &'static str,
}

/// Enumerates the displays attached to the system.
pub trait ScreenService {
    /// All currently connected displays.
    fn displays(&self) -> Result<Vec<Display>>;
    /// The primary display. Fails with [`crate::Error::NoDisplays`] when the
    /// system reports none.
    fn primary(&self) -> Result<Display>;
}

/// Captures pixels from the screen.
pub trait ScreenCapture {
    /// Captures the given rectangle (logical coordinates, global desktop
    /// space), compositing across displays when it spans several.
    fn grab(&self, rect: Rect) -> Result<Frame>;
    /// Captures an entire display by its [`Display::id`].
    fn grab_display(&self, display_id: u32) -> Result<Frame>;
}

/// Queries and requests OS permissions.
pub trait PermissionService {
    /// The current state of a permission, without prompting.
    fn state(&self, permission: Permission) -> PermissionState;
    /// Requests a permission, prompting the user when needed, and returns
    /// the resulting state.
    fn request(&self, permission: Permission) -> Result<PermissionState>;
}

/// Registers global hotkeys with the operating system.
pub trait HotkeyService {
    /// Binds `accelerator` (e.g. `"Cmd+Shift+4"`) to the given identifier.
    fn register(&self, id: &str, accelerator: &str) -> Result<()>;
    /// Removes a previously registered binding.
    fn unregister(&self, id: &str) -> Result<()>;
    /// Whether the bindings are truly global (fire while the app is not
    /// focused).
    fn is_global(&self) -> bool;
}

/// Writes to the system clipboard.
pub trait ClipboardService {
    /// Copies an image to the clipboard.
    fn set_image(&self, frame: &Frame) -> Result<()>;
    /// Copies text to the clipboard.
    fn set_text(&self, text: &str) -> Result<()>;
}

/// Posts user-facing notifications.
pub trait NotificationService {
    /// Shows a notification with a title and a body.
    fn notify(&self, title: &str, body: &str) -> Result<()>;
}

/// Minimal HTTP transport for update checks and uploads.
pub trait Fetch {
    /// Performs a GET and returns the raw response body.
    fn get(&self, url: &str) -> Result<Vec<u8>>;
    /// Performs a PUT with a content type and returns the response body as
    /// text (typically a URL pointing at the uploaded resource).
    fn put(&self, url: &str, body: &[u8], content_type: &str) -> Result<String>;
    /// Performs a PUT with extra request headers — needed for signed
    /// uploads (SigV4 sends `authorization`, `x-amz-date` and
    /// `x-amz-content-sha256` as headers).
    ///
    /// The default fails with [`Error::Unsupported`]; adapters that can send
    /// custom headers override it.
    fn put_with_headers(
        &self,
        url: &str,
        body: &[u8],
        content_type: &str,
        headers: &[(String, String)],
    ) -> Result<String> {
        let _ = (url, body, content_type, headers);
        Err(Error::Unsupported { what: "PUT with custom headers".to_string() })
    }
}

/// Describes native window chrome so the UI can imitate it.
pub trait ChromeStyle {
    /// The window chrome parameters of this platform.
    fn chrome(&self) -> WindowChrome;
    /// A stable lowercase OS name (e.g. `"macos"`, `"linux"`, `"windows"`).
    fn os_name(&self) -> &'static str;
}

/// Well-known directories the app is allowed to read and write.
pub trait Paths {
    /// Where captured images are stored.
    fn captures_dir(&self) -> PathBuf;
    /// Where configuration files are stored.
    fn config_dir(&self) -> PathBuf;
}

/// The complete platform adapter surface.
///
/// An adapter (one per OS) implements exactly this trait set and nothing
/// else — and Core branches on capabilities only through these traits,
/// never on the operating system.
pub trait Platform {
    /// Display enumeration.
    fn screens(&self) -> &dyn ScreenService;
    /// Pixel capture.
    fn capture(&self) -> &dyn ScreenCapture;
    /// Permission queries and requests.
    fn permissions(&self) -> &dyn PermissionService;
    /// Global hotkeys.
    fn hotkeys(&self) -> &dyn HotkeyService;
    /// Clipboard writes.
    fn clipboard(&self) -> &dyn ClipboardService;
    /// User notifications.
    fn notifications(&self) -> &dyn NotificationService;
    /// HTTP transport.
    fn fetch(&self) -> &dyn Fetch;
    /// Window chrome styling.
    fn chrome_style(&self) -> &dyn ChromeStyle;
    /// Well-known directories.
    fn paths(&self) -> &dyn Paths;
}
