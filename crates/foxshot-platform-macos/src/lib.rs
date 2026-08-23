//! foxshot-platform-macos — the FoxShot platform adapter for macOS.
//!
//! Implements the full [`foxshot_core::platform`] trait set against
//! CoreGraphics: display enumeration through `CGGetActiveDisplayList`,
//! `CGDisplayBounds` and the display mode's pixel-to-logical ratio; pixel
//! capture through `CGDisplayCreateImage` with correct BGRA→RGBA conversion
//! and stride handling; screen-recording permission via
//! `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`; and
//! HTTP fetch over rustls (ureq). Hotkeys, clipboard, and notifications are
//! not implemented yet and report [`Error::Unsupported`] naming the slice
//! that adds them — that is honest behaviour, not a placeholder.
//!
//! Two things a reader must know up front:
//!
//! * **`CGDisplayCreateImage` is deprecated since macOS 14.4.** Apple
//!   replaces it with ScreenCaptureKit; that migration is a later slice.
//!   Until then this adapter uses the deprecated API deliberately — it is
//!   still functional on every macOS version FoxShot targets.
//! * **This adapter has never been compiled or run on macOS hardware yet.**
//!   It is written against the core-graphics 0.24 API and reviewed, but the
//!   first real `cargo check --target aarch64-apple-darwin` on a Mac is
//!   still owed. Treat every line below as unverified until that run exists.

#![cfg(target_os = "macos")]

use core_graphics::access::ScreenCaptureAccess;
use core_graphics::display::CGDisplay;
use core_graphics::image::CGImage;
use foxshot_core::error::{Error, Result};
use foxshot_core::frame::{Frame, BYTES_PER_PIXEL};
use foxshot_core::geometry::{Point, Rect, Scale, Size};
use foxshot_core::platform::{
    ButtonSide, ChromeStyle, ClipboardService, Display, Fetch, HotkeyService,
    NotificationService, Paths, Permission, PermissionService, PermissionState, Platform,
    ScreenCapture, ScreenService, WindowChrome,
};
use std::path::PathBuf;
use std::time::Duration;
use ureq::ResponseExt as _;

/// Largest response body fetch will read — a guard against unbounded
/// downloads. Update manifests are a few kilobytes; 8 MiB is generous.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

// HIServices: true when the process is a trusted accessibility client.
// Declared by hand because neither core-graphics nor core-foundation binds
// it; it lives in HIServices, linked through the ApplicationServices
// umbrella framework. C `Boolean` is `unsigned char`, so the return type
// is `c_uchar`, not Rust `bool`.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> std::ffi::c_uchar;
}

/// The macOS platform adapter. Construct with [`MacosPlatform::connect`].
pub struct MacosPlatform {
    /// HTTP agent for [`Fetch`]: rustls, 10s global timeout, ≤5 redirects.
    agent: ureq::Agent,
}

impl std::fmt::Debug for MacosPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacosPlatform").finish_non_exhaustive()
    }
}

/// Maps any displayable transport failure into Core's error vocabulary.
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport { message: error.to_string() }
}

/// Builds the "not implemented yet" error for capabilities of later slices.
fn unsupported(capability: &str, slice: &str) -> Error {
    Error::Unsupported { what: format!("{capability} (lands in slice {slice})") }
}

impl MacosPlatform {
    /// Builds the adapter. There is no display server handshake on macOS —
    /// CoreGraphics calls work without a connection object — so this only
    /// prepares the HTTP agent. It returns `Result` to mirror the Linux
    /// adapter, where connecting can genuinely fail.
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

    /// The backing scale factor of a display: the current mode's pixel width
    /// divided by its logical width (2.0 on Retina). Falls back to 1.0 when
    /// the mode is unavailable or degenerate — a guessed 1.0 is safer than
    /// a fabricated scale.
    fn scale_of(display: &CGDisplay) -> Scale {
        if let Some(mode) = display.display_mode() {
            let logical = mode.width();
            if logical > 0 {
                return Scale::new(mode.pixel_width() as f32 / logical as f32);
            }
        }
        Scale::new(1.0)
    }

    /// Converts one `CGDisplay` into Core's [`Display`] vocabulary.
    fn describe_display(id: u32) -> Display {
        let display = CGDisplay::new(id);
        let bounds = display.bounds();
        Display {
            id,
            // Display names require IOKit, which is not bound yet; use the
            // built-in flag for a meaningful main-display name and a stable
            // synthetic name otherwise.
            name: if display.is_builtin() {
                "Built-in display".to_string()
            } else {
                format!("Display {id}")
            },
            bounds: Rect::from_xywh(
                bounds.origin.x.round() as i32,
                bounds.origin.y.round() as i32,
                bounds.size.width.round() as u32,
                bounds.size.height.round() as u32,
            ),
            scale: Self::scale_of(&display),
            is_primary: display.is_main(),
        }
    }

    /// The union of all display bounds: the whole desktop in logical
    /// coordinates. Fails with [`Error::NoDisplays`] on an empty system.
    fn desktop_bounds(displays: &[Display]) -> Result<Rect> {
        let mut iter = displays.iter();
        let first = iter.next().ok_or(Error::NoDisplays)?;
        Ok(iter.fold(first.bounds, |acc, d| acc.union(&d.bounds)))
    }

    /// Every capture must pass the screen-recording preflight first. Without
    /// the grant, `CGDisplayCreateImage` does not fail — it silently returns
    /// the desktop wallpaper and the user's own windows with every other
    /// window missing, which is exactly how other tools produce a
    /// "screenshot" that is wrong without saying so. Refuse instead.
    fn ensure_capture_permission(&self) -> Result<()> {
        if ScreenCaptureAccess.preflight() {
            Ok(())
        } else {
            Err(Error::PermissionDenied {
                permission: Permission::ScreenCapture.label().to_string(),
            })
        }
    }

    /// Converts a `CGDisplayCreateImage` result into an RGBA8 [`Frame`].
    ///
    /// The image is 32 bits per pixel in BGRA byte order
    /// (`kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`, the
    /// only format `CGDisplayCreateImage` produces on little-endian macOS).
    /// The bytes-per-row stride is read from the image and honoured row by
    /// row — it is padded and must never be assumed to equal width × 4.
    fn image_to_frame(image: &CGImage, scale: Scale) -> Result<Frame> {
        let bits = image.bits_per_pixel();
        if bits != 32 {
            return Err(Error::Unsupported { what: format!("{bits} bits per pixel") });
        }
        let width = image.width();
        let height = image.height();
        let stride = image.bytes_per_row();
        let data = image.data();
        let bytes = data.bytes();
        // The last row is not padded: require stride × (height-1) + width × 4.
        let needed = stride
            .checked_mul(height.saturating_sub(1))
            .and_then(|n| n.checked_add(width * BYTES_PER_PIXEL))
            .ok_or(Error::InvalidPixelBuffer { expected: usize::MAX, got: bytes.len() })?;
        if bytes.len() < needed {
            return Err(Error::InvalidPixelBuffer { expected: needed, got: bytes.len() });
        }
        let mut pixels = vec![0u8; width * height * BYTES_PER_PIXEL];
        for y in 0..height {
            let row = &bytes[y * stride..];
            for x in 0..width {
                let bgra = &row[x * BYTES_PER_PIXEL..x * BYTES_PER_PIXEL + BYTES_PER_PIXEL];
                let offset = (y * width + x) * BYTES_PER_PIXEL;
                pixels[offset] = bgra[2]; // R
                pixels[offset + 1] = bgra[1]; // G
                pixels[offset + 2] = bgra[0]; // B
                pixels[offset + 3] = bgra[3]; // A
            }
        }
        let size = Size { width: width as u32, height: height as u32 };
        Frame::from_rgba8(size, scale, pixels)
    }

    /// Captures one already-resolved display via `CGDisplayCreateImage`.
    /// The caller must have run [`MacosPlatform::ensure_capture_permission`].
    fn capture_display_image(&self, display: &Display) -> Result<Frame> {
        let image = CGDisplay::new(display.id).image().ok_or_else(|| Error::Transport {
            message: format!("CGDisplayCreateImage returned null for display {}", display.id),
        })?;
        Self::image_to_frame(&image, display.scale)
    }
}

/// Copies `src` into the canvas `dst` at `dest` (physical pixels), clipping
/// to the canvas on every side. No resampling: when displays have different
/// scales each crop keeps its own pixel density, so a region spanning
/// mixed-DPI displays is correct per display even though the two halves
/// differ in pixel density.
fn blit(dst: &mut [u8], dst_size: Size, dest: Point, src: &Frame) {
    let src_size = src.size();
    for sy in 0..src_size.height as usize {
        let dy = dest.y + sy as i32;
        if dy < 0 || dy >= dst_size.height as i32 {
            continue;
        }
        let sx_start = usize::try_from(-dest.x).unwrap_or(0);
        if sx_start >= src_size.width as usize {
            continue;
        }
        let dx_start = usize::try_from(dest.x).unwrap_or(0);
        if dx_start >= dst_size.width as usize {
            continue;
        }
        let count = (src_size.width as usize - sx_start).min(dst_size.width as usize - dx_start);
        let src_start = (sy * src_size.width as usize + sx_start) * BYTES_PER_PIXEL;
        let dst_start = (dy as usize * dst_size.width as usize + dx_start) * BYTES_PER_PIXEL;
        dst[dst_start..dst_start + count * BYTES_PER_PIXEL]
            .copy_from_slice(&src.bytes()[src_start..src_start + count * BYTES_PER_PIXEL]);
    }
}

impl ScreenService for MacosPlatform {
    /// All active displays via `CGGetActiveDisplayList`; the main display
    /// (`CGMainDisplayID`, reported by `CGDisplayIsMain`) is the primary.
    fn displays(&self) -> Result<Vec<Display>> {
        let ids = CGDisplay::active_displays().map_err(transport)?;
        Ok(ids.into_iter().map(MacosPlatform::describe_display).collect())
    }

    fn primary(&self) -> Result<Display> {
        self.displays()?.into_iter().find(|d| d.is_primary).ok_or(Error::NoDisplays)
    }
}

impl ScreenCapture for MacosPlatform {
    /// Captures `rect` (logical, global desktop space), compositing across
    /// displays. Each intersecting display is captured whole via
    /// `CGDisplayCreateImage` and the overlap is cropped out of the captured
    /// image in that display's own physical pixels. The canvas uses the
    /// scale of the display containing the rect's origin; mixed-DPI spans
    /// are blitted without resampling (see [`blit`]).
    fn grab(&self, rect: Rect) -> Result<Frame> {
        self.ensure_capture_permission()?;
        let displays = self.displays()?;
        let anchor = displays
            .iter()
            .find(|d| d.bounds.contains_point(rect.origin))
            .or_else(|| displays.iter().find(|d| d.is_primary))
            .ok_or(Error::NoDisplays)?;
        let canvas_scale = anchor.scale;
        let canvas_size = canvas_scale.to_physical(rect.size);
        let desktop = Self::desktop_bounds(&displays)?;
        if canvas_size.width == 0 || canvas_size.height == 0 {
            return Err(Error::RectOutOfBounds { requested: rect, bounds: desktop });
        }

        let mut pixels =
            vec![0u8; canvas_size.width as usize * canvas_size.height as usize * BYTES_PER_PIXEL];
        let mut covered = false;
        for display in &displays {
            let Some(overlap) = rect.intersection(&display.bounds) else { continue };
            covered = true;
            let full = self.capture_display_image(display)?;
            // The overlap in the display's own coordinate space, in physical
            // pixels, clamped to the captured image so scale rounding can
            // never push the crop outside the frame.
            let local = Rect::from_xywh(
                overlap.left() - display.bounds.left(),
                overlap.top() - display.bounds.top(),
                overlap.size.width,
                overlap.size.height,
            );
            let physical = display.scale.rect_to_physical(local);
            let frame_bounds = Rect::from_xywh(0, 0, full.size().width, full.size().height);
            let Some(crop_rect) = physical.intersection(&frame_bounds) else { continue };
            if crop_rect.is_empty() {
                continue;
            }
            let cropped = full.crop(crop_rect)?;
            let dest = canvas_scale.point_to_physical(Point {
                x: overlap.left() - rect.left(),
                y: overlap.top() - rect.top(),
            });
            blit(&mut pixels, canvas_size, dest, &cropped);
        }
        if !covered {
            return Err(Error::RectOutOfBounds { requested: rect, bounds: desktop });
        }
        Frame::from_rgba8(canvas_size, canvas_scale, pixels)
    }

    fn grab_display(&self, display_id: u32) -> Result<Frame> {
        self.ensure_capture_permission()?;
        let displays = self.displays()?;
        let display = displays
            .iter()
            .find(|d| d.id == display_id)
            .ok_or_else(|| Error::Unsupported { what: format!("unknown display id {display_id}") })?;
        self.capture_display_image(display)
    }
}

impl PermissionService for MacosPlatform {
    /// `CGPreflightScreenCaptureAccess` is the only supported way to ask
    /// about screen recording without prompting. Its `false` cannot
    /// distinguish "never asked" from "denied in System Settings" — macOS
    /// exposes no API for that difference — so `false` maps to
    /// [`PermissionState::NotRequested`]. Accessibility uses the hand-bound
    /// `AXIsProcessTrusted` (HIServices). The clipboard has no permission
    /// gate on macOS and is always granted.
    fn state(&self, permission: Permission) -> PermissionState {
        match permission {
            Permission::ScreenCapture => {
                if ScreenCaptureAccess.preflight() {
                    PermissionState::Granted
                } else {
                    PermissionState::NotRequested
                }
            }
            // Safety: AXIsProcessTrusted is a pure query with no side
            // effects; its `Boolean` result is 0 or 1.
            Permission::Accessibility => {
                if unsafe { AXIsProcessTrusted() } != 0 {
                    PermissionState::Granted
                } else {
                    PermissionState::NotRequested
                }
            }
            Permission::Clipboard => PermissionState::Granted,
        }
    }

    /// `CGRequestScreenCaptureAccess` triggers the system prompt flow and
    /// returns whether capture is now permitted; a still-false result is
    /// reported as [`PermissionState::Denied`] because after an explicit
    /// request the remaining states are indistinguishable from denial.
    /// There is no supported programmatic prompt for Accessibility trust
    /// without `AXIsProcessTrustedWithOptions` and its CFDictionary options —
    /// that lands together with the global-hotkey slice S7 that needs it —
    /// so `request` returns the current state unchanged.
    fn request(&self, permission: Permission) -> Result<PermissionState> {
        match permission {
            Permission::ScreenCapture => {
                if ScreenCaptureAccess.request() {
                    Ok(PermissionState::Granted)
                } else {
                    Ok(PermissionState::Denied)
                }
            }
            Permission::Accessibility => Ok(self.state(permission)),
            Permission::Clipboard => Ok(PermissionState::Granted),
        }
    }
}

impl HotkeyService for MacosPlatform {
    fn register(&self, _id: &str, _accelerator: &str) -> Result<()> {
        Err(unsupported("global hotkeys", "S7"))
    }

    fn unregister(&self, _id: &str) -> Result<()> {
        Err(unsupported("global hotkeys", "S7"))
    }

    fn is_global(&self) -> bool {
        false
    }
}

impl ClipboardService for MacosPlatform {
    fn set_image(&self, _frame: &Frame) -> Result<()> {
        Err(unsupported("clipboard", "S7"))
    }

    fn set_text(&self, _text: &str) -> Result<()> {
        Err(unsupported("clipboard", "S7"))
    }
}

impl NotificationService for MacosPlatform {
    fn notify(&self, _title: &str, _body: &str) -> Result<()> {
        Err(unsupported("notifications", "S7"))
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

impl Fetch for MacosPlatform {
    /// HTTPS GET: at most 5 redirects and a 10 second global timeout (agent
    /// configuration), at most [`MAX_BODY_BYTES`] read here. Transport
    /// failures and non-2xx statuses both become [`Error::Transport`].
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        let mut response = self.agent.get(url).call().map_err(|error| Error::Transport {
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
        let response = self
            .agent
            .put(url)
            .header("Content-Type", content_type)
            .send(body)
            .map_err(|error| Error::Transport {
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

impl ChromeStyle for MacosPlatform {
    /// macOS chrome: the traffic-light buttons sit on the left, windows have
    /// a generous corner radius, and the primary modifier is Command.
    fn chrome(&self) -> WindowChrome {
        WindowChrome {
            buttons: ButtonSide::Left,
            corner_radius: 14.0,
            titlebar_height: 42.0,
            modifier_label: "Cmd",
        }
    }

    fn os_name(&self) -> &'static str {
        "macOS"
    }
}

impl Paths for MacosPlatform {
    /// `~/Documents/FoxShot` — where Mac users expect screenshots to land.
    fn captures_dir(&self) -> PathBuf {
        home().join("Documents").join("FoxShot")
    }

    /// `~/Library/Application Support/foxshot` — the macOS config location.
    fn config_dir(&self) -> PathBuf {
        home().join("Library").join("Application Support").join("foxshot")
    }
}

/// `$HOME`, falling back to the filesystem root when it is unset.
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

impl Platform for MacosPlatform {
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
