//! foxshot-platform-windows — the FoxShot platform adapter for Windows.
//!
//! A minimal, honest adapter: it exists so the workspace — and therefore
//! `foxshot-app` — compiles for Windows, while being truthful about what it
//! can and cannot do.
//!
//! * **Real:** window-chrome values (Windows 11 metrics), well-known paths
//!   (`%USERPROFILE%\Documents\FoxShot`, `%APPDATA%\foxshot`), and HTTP fetch
//!   over the same rustls ureq stack the Linux and macOS adapters use.
//! * **Truthful:** Windows has no screen-capture or clipboard permission
//!   gate, so those permissions report [`PermissionState::Granted`] — that is
//!   a true statement about the platform, not a fake grant. Accessibility is
//!   [`PermissionState::NotRequested`] because nothing here asks for it.
//! * **Not yet:** display enumeration, pixel capture, global hotkeys,
//!   clipboard writes and notifications return [`Error::Unsupported`] naming
//!   the tracking issue ("Phase: Windows platform adapter — issue #6").
//!   Nothing here fabricates a capture.
//!
//! **This adapter has not been compiled on Windows hardware yet** — the
//! first real `cargo check --target x86_64-pc-windows-gnu` in CI is still the
//! verification that counts.

#![cfg(target_os = "windows")]

use foxshot_core::error::{Error, Result};
use foxshot_core::frame::Frame;
use foxshot_core::geometry::Rect;
use foxshot_core::platform::{
    ButtonSide, ChromeStyle, ClipboardService, Display, Fetch, HotkeyService, NotificationService,
    Paths, Permission, PermissionService, PermissionState, Platform, ScreenCapture, ScreenService,
    WindowChrome,
};
use std::path::PathBuf;
use std::time::Duration;
use ureq::ResponseExt as _;

/// Largest response body fetch will read — a guard against unbounded
/// downloads. Update manifests are a few kilobytes; 8 MiB is generous.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// The Windows platform adapter. Construct with [`WindowsPlatform::connect`].
pub struct WindowsPlatform {
    /// HTTP agent for [`Fetch`]: rustls, 10s global timeout, ≤5 redirects.
    agent: ureq::Agent,
}

impl std::fmt::Debug for WindowsPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsPlatform").finish_non_exhaustive()
    }
}

/// Builds the "not implemented yet" error for the capabilities the Windows
/// adapter does not have yet. The message names the tracking issue and its
/// phase so a user hitting it learns exactly where the work is tracked.
fn unsupported(capability: &str) -> Error {
    Error::Unsupported {
        what: format!("{capability} (Phase: Windows platform adapter — issue #6)"),
    }
}

impl WindowsPlatform {
    /// Builds the adapter. There is no connection handshake here — this only
    /// prepares the HTTP agent. It returns `Result` to mirror the other
    /// adapters, where connecting can genuinely fail.
    pub fn connect() -> Result<Self> {
        // Status codes are mapped into Core errors by hand, so ureq must not
        // convert them into its own error kind first.
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(10)))
                .max_redirects(5)
                .http_status_as_error(false)
                .build(),
        );
        Ok(Self { agent })
    }
}

impl ScreenService for WindowsPlatform {
    /// Display enumeration needs the Win32 `EnumDisplayMonitors` API, which
    /// is not bound yet — refuse honestly instead of inventing a display.
    fn displays(&self) -> Result<Vec<Display>> {
        Err(unsupported("display enumeration"))
    }

    fn primary(&self) -> Result<Display> {
        Err(unsupported("display enumeration"))
    }
}

impl ScreenCapture for WindowsPlatform {
    /// No faked pixels: capturing needs the Windows Graphics Capture API
    /// (or GDI BitBlt), which this adapter does not have yet.
    fn grab(&self, _rect: Rect) -> Result<Frame> {
        Err(unsupported("screen capture"))
    }

    fn grab_display(&self, _display_id: u32) -> Result<Frame> {
        Err(unsupported("screen capture"))
    }
}

impl PermissionService for WindowsPlatform {
    /// Windows gates neither screen capture nor clipboard access, so both
    /// report [`PermissionState::Granted`] — a true statement about the
    /// platform. Accessibility is not requested: the adapter never asks for
    /// it, and without global hotkeys it never needs to.
    fn state(&self, permission: Permission) -> PermissionState {
        match permission {
            Permission::ScreenCapture | Permission::Clipboard => PermissionState::Granted,
            Permission::Accessibility => PermissionState::NotRequested,
        }
    }

    /// There is nothing to prompt for on Windows: requesting returns the
    /// current state unchanged.
    fn request(&self, permission: Permission) -> Result<PermissionState> {
        Ok(self.state(permission))
    }
}

impl HotkeyService for WindowsPlatform {
    fn register(&self, _id: &str, _accelerator: &str) -> Result<()> {
        Err(unsupported("global hotkeys"))
    }

    fn unregister(&self, _id: &str) -> Result<()> {
        Err(unsupported("global hotkeys"))
    }

    fn is_global(&self) -> bool {
        false
    }
}

impl ClipboardService for WindowsPlatform {
    fn set_image(&self, _frame: &Frame) -> Result<()> {
        Err(unsupported("clipboard"))
    }

    fn set_text(&self, _text: &str) -> Result<()> {
        Err(unsupported("clipboard"))
    }
}

impl NotificationService for WindowsPlatform {
    fn notify(&self, _title: &str, _body: &str) -> Result<u32> {
        Err(unsupported("notifications"))
    }
}

/// Adapter crate version, from the package manifest at build time. The app
/// registers the adapter under this version in the module registry.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The host part of a URL, for error messages: `https://host/path` → `host`.
fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| !host.is_empty())
        .unwrap_or(url)
}

impl Fetch for WindowsPlatform {
    /// HTTPS GET: at most 5 redirects and a 10 second global timeout (agent
    /// configuration), at most [`MAX_BODY_BYTES`] read here. Transport
    /// failures and non-2xx statuses both become [`Error::Transport`].
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|error| Error::Transport {
                message: format!("GET to {} failed: {error}", host_of(url)),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Transport {
                message: format!("GET to {} returned status {status}", host_of(url)),
            });
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|error| Error::Transport {
                message: format!(
                    "reading body from {} failed (limit {} MiB): {error}",
                    host_of(url),
                    MAX_BODY_BYTES / (1024 * 1024)
                ),
            })
    }

    /// HTTP PUT with the given content type; on 2xx returns the final URL
    /// after redirects. Same error mapping as [`Fetch::get`].
    fn put(&self, url: &str, body: &[u8], content_type: &str) -> Result<String> {
        self.put_with_headers(url, body, content_type, &[])
    }

    /// HTTP PUT with extra request headers (SigV4 signed uploads). Same
    /// error mapping as [`Fetch::get`].
    fn put_with_headers(
        &self,
        url: &str,
        body: &[u8],
        content_type: &str,
        headers: &[(String, String)],
    ) -> Result<String> {
        let mut request = self.agent.put(url).header("Content-Type", content_type);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send(body).map_err(|error| Error::Transport {
            message: format!("PUT to {} failed: {error}", host_of(url)),
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Transport {
                message: format!("PUT to {} returned status {status}", host_of(url)),
            });
        }
        Ok(response.get_uri().to_string())
    }
}

impl ChromeStyle for WindowsPlatform {
    /// Windows 11 chrome: the window buttons sit on the right, windows have
    /// an 8-point corner radius, the titlebar is 32 points tall, and the
    /// primary modifier is Ctrl.
    fn chrome(&self) -> WindowChrome {
        WindowChrome {
            buttons: ButtonSide::Right,
            corner_radius: 8.0,
            titlebar_height: 32.0,
            modifier_label: "Ctrl",
        }
    }

    fn os_name(&self) -> &'static str {
        "Windows"
    }
}

impl Paths for WindowsPlatform {
    /// `%USERPROFILE%\Documents\FoxShot` — where Windows users expect
    /// documents, and therefore screenshots, to land.
    fn captures_dir(&self) -> PathBuf {
        user_profile().join("Documents").join("FoxShot")
    }

    /// `%APPDATA%\foxshot` — the roaming per-user configuration location.
    /// Falls back under `%USERPROFILE%\AppData\Roaming` when `APPDATA` is
    /// unset (every interactive Windows session sets it).
    fn config_dir(&self) -> PathBuf {
        std::env::var_os("APPDATA")
            .map_or_else(
                || user_profile().join("AppData").join("Roaming"),
                PathBuf::from,
            )
            .join("foxshot")
    }
}

/// `%USERPROFILE%`, falling back to the system drive root when it is unset.
fn user_profile() -> PathBuf {
    std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from(r"C:\"), PathBuf::from)
}

impl Platform for WindowsPlatform {
    fn screens(&self) -> &dyn ScreenService {
        self
    }
    fn capture(&self) -> &dyn ScreenCapture {
        self
    }
    fn permissions(&self) -> &dyn PermissionService {
        self
    }
    fn hotkeys(&self) -> &dyn HotkeyService {
        self
    }
    fn clipboard(&self) -> &dyn ClipboardService {
        self
    }
    fn notifications(&self) -> &dyn NotificationService {
        self
    }
    fn fetch(&self) -> &dyn Fetch {
        self
    }
    fn chrome_style(&self) -> &dyn ChromeStyle {
        self
    }
    fn paths(&self) -> &dyn Paths {
        self
    }
}
