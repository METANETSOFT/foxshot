//! Test-only infrastructure.
//!
//! [`NullPlatform`] is a deterministic, fully in-memory implementation of
//! the entire [`Platform`] trait set. It exists so Core behaviour can be
//! unit-tested without an operating system: it is honest test scaffolding
//! (it records what was asked of it and answers with fixed, documented
//! data), **not** a stub of production behaviour, and it is never wired
//! into the shipped application.

use crate::error::{Error, Result};
use crate::frame::Frame;
use crate::geometry::{Rect, Scale};
use crate::platform::{
    ButtonSide, ChromeStyle, ClipboardService, Display, Fetch, HotkeyService,
    NotificationService, Paths, Permission, PermissionService, PermissionState, Platform,
    ScreenCapture, ScreenService, WindowChrome,
};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

/// A deterministic in-memory platform for tests.
///
/// Fixed behaviour:
/// - two displays: 2560x1440 @1.0 primary at (0,0), 1920x1080 @2.0 at (2560,0);
/// - permissions start [`PermissionState::NotRequested`] and flip to
///   [`PermissionState::Granted`] when requested;
/// - [`ScreenCapture::grab`] returns a solid-colour frame of the requested size;
/// - clipboard and notification calls are recorded in readable counters;
/// - [`Fetch::get`] returns the bytes handed to the constructor, and
///   [`Fetch::put`] returns a fixed URL string.
#[derive(Debug)]
pub struct NullPlatform {
    fetch_body: Vec<u8>,
    permissions: RefCell<Vec<(Permission, PermissionState)>>,
    registered_hotkeys: RefCell<Vec<(String, String)>>,
    clipboard_images: Cell<usize>,
    clipboard_texts: RefCell<Vec<String>>,
    notifications: RefCell<Vec<(String, String)>>,
}

impl NullPlatform {
    /// Body returned by every [`Fetch::put`] call.
    pub const PUT_RESPONSE: &'static str = "https://captures.example.invalid/null";

    /// Creates a null platform whose `Fetch::get` returns `fetch_body`.
    pub fn new(fetch_body: Vec<u8>) -> Self {
        Self {
            fetch_body,
            permissions: RefCell::new(Vec::new()),
            registered_hotkeys: RefCell::new(Vec::new()),
            clipboard_images: Cell::new(0),
            clipboard_texts: RefCell::new(Vec::new()),
            notifications: RefCell::new(Vec::new()),
        }
    }

    /// Creates a null platform whose `Fetch::get` returns an empty body.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// The two fixed displays this platform reports.
    fn fixed_displays() -> Vec<Display> {
        vec![
            Display {
                id: 1,
                name: "Null Primary".to_string(),
                bounds: Rect::from_xywh(0, 0, 2560, 1440),
                scale: Scale::new(1.0),
                is_primary: true,
            },
            Display {
                id: 2,
                name: "Null Secondary".to_string(),
                bounds: Rect::from_xywh(2560, 0, 1920, 1080),
                scale: Scale::new(2.0),
                is_primary: false,
            },
        ]
    }

    /// How many images were written to the clipboard so far.
    pub fn clipboard_image_count(&self) -> usize {
        self.clipboard_images.get()
    }

    /// Every text written to the clipboard so far, in order.
    pub fn clipboard_texts(&self) -> Vec<String> {
        self.clipboard_texts.borrow().clone()
    }

    /// Every `(title, body)` notification posted so far, in order.
    pub fn notification_log(&self) -> Vec<(String, String)> {
        self.notifications.borrow().clone()
    }

    /// Every `(id, accelerator)` hotkey currently registered.
    pub fn registered_hotkeys(&self) -> Vec<(String, String)> {
        self.registered_hotkeys.borrow().clone()
    }
}

impl Default for NullPlatform {
    fn default() -> Self {
        Self::empty()
    }
}

impl ScreenService for NullPlatform {
    fn displays(&self) -> Result<Vec<Display>> {
        Ok(Self::fixed_displays())
    }

    fn primary(&self) -> Result<Display> {
        Self::fixed_displays()
            .into_iter()
            .find(|d| d.is_primary)
            .ok_or(Error::NoDisplays)
    }
}

impl ScreenCapture for NullPlatform {
    fn grab(&self, rect: Rect) -> Result<Frame> {
        Ok(Frame::new_filled(rect.size, Scale::new(1.0), [0x33, 0x66, 0x99, 0xFF]))
    }

    fn grab_display(&self, display_id: u32) -> Result<Frame> {
        let display = Self::fixed_displays()
            .into_iter()
            .find(|d| d.id == display_id)
            .ok_or_else(|| Error::Unsupported {
                what: format!("display id {display_id}"),
            })?;
        Ok(Frame::new_filled(display.bounds.size, display.scale, [0x33, 0x66, 0x99, 0xFF]))
    }
}

impl PermissionService for NullPlatform {
    fn state(&self, permission: Permission) -> PermissionState {
        self.permissions
            .borrow()
            .iter()
            .find(|(p, _)| *p == permission)
            .map(|(_, s)| *s)
            .unwrap_or(PermissionState::NotRequested)
    }

    fn request(&self, permission: Permission) -> Result<PermissionState> {
        let mut states = self.permissions.borrow_mut();
        states.retain(|(p, _)| *p != permission);
        states.push((permission, PermissionState::Granted));
        Ok(PermissionState::Granted)
    }
}

impl HotkeyService for NullPlatform {
    fn register(&self, id: &str, accelerator: &str) -> Result<()> {
        self.registered_hotkeys
            .borrow_mut()
            .push((id.to_string(), accelerator.to_string()));
        Ok(())
    }

    fn unregister(&self, id: &str) -> Result<()> {
        self.registered_hotkeys.borrow_mut().retain(|(k, _)| k != id);
        Ok(())
    }

    fn is_global(&self) -> bool {
        true
    }
}

impl ClipboardService for NullPlatform {
    fn set_image(&self, frame: &Frame) -> Result<()> {
        let _ = frame;
        self.clipboard_images.set(self.clipboard_images.get() + 1);
        Ok(())
    }

    fn set_text(&self, text: &str) -> Result<()> {
        self.clipboard_texts.borrow_mut().push(text.to_string());
        Ok(())
    }
}

impl NotificationService for NullPlatform {
    fn notify(&self, title: &str, body: &str) -> Result<u32> {
        self.notifications
            .borrow_mut()
            .push((title.to_string(), body.to_string()));
        Ok(0)
    }
}

impl Fetch for NullPlatform {
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        let _ = url;
        Ok(self.fetch_body.clone())
    }

    fn put(&self, url: &str, body: &[u8], content_type: &str) -> Result<String> {
        let _ = (url, body, content_type);
        Ok(Self::PUT_RESPONSE.to_string())
    }

    /// Signed PUTs answer exactly like plain ones — the null platform
    /// records nothing it cannot verify.
    fn put_with_headers(
        &self,
        url: &str,
        body: &[u8],
        content_type: &str,
        headers: &[(String, String)],
    ) -> Result<String> {
        let _ = headers;
        self.put(url, body, content_type)
    }
}

impl ChromeStyle for NullPlatform {
    fn chrome(&self) -> WindowChrome {
        WindowChrome {
            buttons: ButtonSide::Right,
            corner_radius: 8.0,
            titlebar_height: 28.0,
            modifier_label: "Ctrl",
        }
    }

    fn os_name(&self) -> &'static str {
        "null"
    }
}

impl Paths for NullPlatform {
    fn captures_dir(&self) -> PathBuf {
        PathBuf::from("/tmp/foxshot-null/captures")
    }

    fn config_dir(&self) -> PathBuf {
        PathBuf::from("/tmp/foxshot-null/config")
    }
}

impl Platform for NullPlatform {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Size;

    #[test]
    fn permissions_flip_from_not_requested_to_granted() {
        let platform = NullPlatform::default();
        let service = platform.permissions();
        assert_eq!(service.state(Permission::ScreenCapture), PermissionState::NotRequested);
        assert_eq!(service.state(Permission::Clipboard), PermissionState::NotRequested);
        let after = service.request(Permission::ScreenCapture).unwrap();
        assert_eq!(after, PermissionState::Granted);
        assert_eq!(service.state(Permission::ScreenCapture), PermissionState::Granted);
        // Untouched permissions stay NotRequested.
        assert_eq!(service.state(Permission::Clipboard), PermissionState::NotRequested);
    }

    #[test]
    fn displays_are_two_with_exactly_one_primary() {
        let platform = NullPlatform::default();
        let displays = platform.screens().displays().unwrap();
        assert_eq!(displays.len(), 2);
        assert_eq!(displays.iter().filter(|d| d.is_primary).count(), 1);
        let primary = platform.screens().primary().unwrap();
        assert!(primary.is_primary);
        assert_eq!(primary.bounds, Rect::from_xywh(0, 0, 2560, 1440));
    }

    #[test]
    fn grab_returns_solid_frame_of_requested_size() {
        let platform = NullPlatform::default();
        let frame = platform.capture().grab(Rect::from_xywh(10, 10, 40, 30)).unwrap();
        assert_eq!(frame.size(), Size { width: 40, height: 30 });
        assert!(frame.bytes().chunks_exact(4).all(|c| c == [0x33, 0x66, 0x99, 0xFF]));
    }

    #[test]
    fn clipboard_and_notifications_record_calls() {
        let platform = NullPlatform::default();
        let frame = Frame::new_filled(Size { width: 2, height: 2 }, Scale::new(1.0), [0, 0, 0, 0]);
        platform.clipboard().set_image(&frame).unwrap();
        platform.clipboard().set_image(&frame).unwrap();
        platform.clipboard().set_text("hello").unwrap();
        platform.notifications().notify("t", "b").unwrap();
        assert_eq!(platform.clipboard_image_count(), 2);
        assert_eq!(platform.clipboard_texts(), vec!["hello".to_string()]);
        assert_eq!(platform.notification_log(), vec![("t".to_string(), "b".to_string())]);
    }

    #[test]
    fn fetch_get_returns_constructor_bytes_and_put_returns_fixed_url() {
        let platform = NullPlatform::new(b"body".to_vec());
        assert_eq!(platform.fetch().get("https://example.invalid").unwrap(), b"body");
        let put = platform
            .fetch()
            .put("https://example.invalid/up", b"x", "image/png")
            .unwrap();
        assert_eq!(put, NullPlatform::PUT_RESPONSE);
    }

    #[test]
    fn hotkeys_register_and_unregister() {
        let platform = NullPlatform::default();
        platform.hotkeys().register("capture", "Ctrl+Shift+4").unwrap();
        assert_eq!(
            platform.registered_hotkeys(),
            vec![("capture".to_string(), "Ctrl+Shift+4".to_string())]
        );
        assert!(platform.hotkeys().is_global());
        platform.hotkeys().unregister("capture").unwrap();
        assert!(platform.registered_hotkeys().is_empty());
    }

    #[test]
    fn permission_labels_are_stable() {
        assert_eq!(Permission::ScreenCapture.label(), "screen-capture");
        assert_eq!(Permission::Accessibility.label(), "accessibility");
        assert_eq!(Permission::Clipboard.label(), "clipboard");
    }
}
