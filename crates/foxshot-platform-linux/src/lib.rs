//! foxshot-platform-linux — the FoxShot platform adapter for Linux/X11.
//!
//! Implements the full [`foxshot_core::platform`] trait set against a real X
//! server: display enumeration through RandR (with a root-window fallback),
//! pixel capture through MIT-SHM shared memory when the server offers it and
//! `GetImage` otherwise. Hotkeys, clipboard, notifications, and HTTP fetch are
//! not implemented yet and report [`Error::Unsupported`] naming the slice that
//! adds them — that is honest behaviour, not a placeholder.

use foxshot_core::error::{Error, Result};
use foxshot_core::frame::{Frame, BYTES_PER_PIXEL};
use foxshot_core::geometry::{Rect, Scale, Size};
use foxshot_core::platform::{
    ButtonSide, ChromeStyle, ClipboardService, Display, Fetch, HotkeyService,
    NotificationService, Paths, Permission, PermissionService, PermissionState, Platform,
    ScreenCapture, ScreenService, WindowChrome,
};
use std::path::PathBuf;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::{randr, shm, xproto};
use x11rb::rust_connection::RustConnection;

/// The Linux/X11 platform adapter. Construct with [`LinuxPlatform::connect`].
pub struct LinuxPlatform {
    conn: RustConnection,
    screen_num: usize,
    /// Whether the server offers a usable MIT-SHM extension.
    shm: bool,
}

impl std::fmt::Debug for LinuxPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxPlatform")
            .field("screen_num", &self.screen_num)
            .field("shm", &self.shm)
            .finish_non_exhaustive()
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

impl LinuxPlatform {
    /// Connects to the X server named by `$DISPLAY` and probes for MIT-SHM.
    pub fn connect() -> Result<Self> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|error| Error::Transport {
            message: format!("cannot connect to X server: {error}"),
        })?;
        let shm = match conn.extension_information(shm::X11_EXTENSION_NAME) {
            Ok(Some(_)) => {
                shm::query_version(&conn).map(|cookie| cookie.reply().is_ok()).unwrap_or(false)
            }
            _ => false,
        };
        Ok(Self { conn, screen_num, shm })
    }

    /// The root window of the screen this connection drives.
    fn root(&self) -> u32 {
        self.conn.setup().roots[self.screen_num].root
    }

    /// The root window bounds in pixels: the whole desktop on X11.
    fn root_bounds(&self) -> Rect {
        let screen = &self.conn.setup().roots[self.screen_num];
        Rect::from_xywh(0, 0, screen.width_in_pixels as u32, screen.height_in_pixels as u32)
    }

    /// Fallback display used when RandR is absent or reports nothing: the X
    /// root window presented as a single primary display.
    fn root_display(&self) -> Display {
        Display {
            id: self.root(),
            name: "X11 root window".to_string(),
            bounds: self.root_bounds(),
            scale: Scale::new(1.0),
            is_primary: true,
        }
    }

    /// Enumerates displays via RandR 1.2 CRTC/output info. Returns
    /// [`Error::Unsupported`] when the server has no RandR, so the caller can
    /// fall back to the root window.
    fn randr_displays(&self) -> Result<Vec<Display>> {
        let root = self.root();
        if self
            .conn
            .extension_information(randr::X11_EXTENSION_NAME)
            .map_err(transport)?
            .is_none()
        {
            return Err(Error::Unsupported { what: "RANDR extension".to_string() });
        }
        // CRTC-based enumeration requires at least RandR 1.2.
        randr::query_version(&self.conn, 1, 2)
            .map_err(transport)?
            .reply()
            .map_err(transport)?;
        let resources = randr::get_screen_resources_current(&self.conn, root)
            .map_err(transport)?
            .reply()
            .map_err(transport)?;
        let primary_output = match randr::get_output_primary(&self.conn, root) {
            Ok(cookie) => cookie.reply().map(|reply| reply.output).unwrap_or(0),
            Err(_) => 0,
        };
        let mut displays = Vec::new();
        for &crtc in &resources.crtcs {
            let info = match randr::get_crtc_info(&self.conn, crtc, resources.config_timestamp) {
                Ok(cookie) => match cookie.reply() {
                    Ok(info) => info,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            if info.width == 0 || info.height == 0 {
                continue; // disabled CRTC
            }
            let is_primary = primary_output != 0 && info.outputs.contains(&primary_output);
            displays.push(Display {
                id: crtc,
                name: self.crtc_name(&info, resources.config_timestamp, crtc),
                bounds: Rect::from_xywh(
                    i32::from(info.x),
                    i32::from(info.y),
                    u32::from(info.width),
                    u32::from(info.height),
                ),
                // X11 has no per-display scale factor; RandR reports none.
                scale: Scale::new(1.0),
                is_primary,
            });
        }
        // A server with no designated primary output: the first display is the
        // de facto primary, so `primary()` never fails on a working desktop.
        if !displays.is_empty() && !displays.iter().any(|d| d.is_primary) {
            displays[0].is_primary = true;
        }
        Ok(displays)
    }

    /// Human-readable name for a CRTC: the name of its first connected output,
    /// or a stable synthetic name when none is connected.
    fn crtc_name(&self, info: &randr::GetCrtcInfoReply, timestamp: u32, crtc: u32) -> String {
        for &output in &info.outputs {
            if let Ok(cookie) = randr::get_output_info(&self.conn, output, timestamp) {
                if let Ok(output_info) = cookie.reply() {
                    if output_info.connection == randr::Connection::CONNECTED
                        && !output_info.name.is_empty()
                    {
                        return String::from_utf8_lossy(&output_info.name).into_owned();
                    }
                }
            }
        }
        format!("CRTC-{crtc}")
    }

    /// The server's pixmap format (bits per pixel, scanline pad) for a depth.
    fn pixmap_format(&self, depth: u8) -> Result<xproto::Format> {
        self.conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|format| format.depth == depth)
            .copied()
            .ok_or_else(|| Error::Unsupported { what: format!("depth {depth}") })
    }

    /// RGB channel masks of a visual, looked up in the connection setup.
    fn visual_masks(&self, visual: u32) -> Result<(u32, u32, u32)> {
        for allowed_depth in &self.conn.setup().roots[self.screen_num].allowed_depths {
            for visual_type in &allowed_depth.visuals {
                if visual_type.visual_id == visual {
                    return Ok((
                        visual_type.red_mask,
                        visual_type.green_mask,
                        visual_type.blue_mask,
                    ));
                }
            }
        }
        Err(Error::Unsupported { what: format!("visual {visual:#x}") })
    }

    /// Bytes per scanline of a ZPixmap image of `width` pixels in `format`.
    fn stride_bytes(width: u32, format: &xproto::Format) -> usize {
        let bits = width as usize * format.bits_per_pixel as usize;
        let pad = format.scanline_pad as usize;
        bits.div_ceil(pad) * (pad / 8)
    }

    /// Captures `rect` (already clipped to the root window) via MIT-SHM.
    fn grab_shm(&self, rect: Rect) -> Result<Frame> {
        let screen = &self.conn.setup().roots[self.screen_num];
        let format = self.pixmap_format(screen.root_depth)?;
        let stride = Self::stride_bytes(rect.size.width, &format);
        let size = stride * rect.size.height as usize;

        // Classic SysV shared memory, registered with the server via shm_attach.
        // Safety: all three calls are plain libc; `addr` is checked against the
        // (void *)-1 error return before any use, and every path below detaches
        // and removes the segment exactly once.
        let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600) };
        if shmid < 0 {
            return Err(Error::Transport { message: "shmget failed".to_string() });
        }
        let addr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
        if addr.is_null() || addr as isize == -1 {
            unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };
            return Err(Error::Transport { message: "shmat failed".to_string() });
        }

        let captured = (|| {
            let seg = self.conn.generate_id().map_err(transport)?;
            shm::attach(&self.conn, seg, shmid as u32, false)
                .map_err(transport)?
                .check()
                .map_err(transport)?;
            let reply = shm::get_image(
                &self.conn,
                self.root(),
                rect.left() as i16,
                rect.top() as i16,
                rect.size.width as u16,
                rect.size.height as u16,
                !0,
                xproto::ImageFormat::Z_PIXMAP.into(),
                seg,
                0,
            )
            .map_err(transport)?
            .reply()
            .map_err(transport)?;
            // The server has finished writing once the reply arrived.
            let data = unsafe { std::slice::from_raw_parts(addr.cast::<u8>(), size) }.to_vec();
            let _ = shm::detach(&self.conn, seg);
            Ok((reply.depth, reply.visual, data))
        })();

        unsafe {
            libc::shmdt(addr);
            libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut());
        }
        let (depth, visual, data) = captured?;
        self.image_to_rgba(rect.size, depth, visual, &data)
    }

    /// Captures `rect` (already clipped to the root window) via `GetImage`.
    fn grab_get_image(&self, rect: Rect) -> Result<Frame> {
        let reply = xproto::get_image(
            &self.conn,
            xproto::ImageFormat::Z_PIXMAP,
            self.root(),
            rect.left() as i16,
            rect.top() as i16,
            rect.size.width as u16,
            rect.size.height as u16,
            !0,
        )
        .map_err(transport)?
        .reply()
        .map_err(transport)?;
        self.image_to_rgba(rect.size, reply.depth, reply.visual, &reply.data)
    }

    /// Converts a ZPixmap image in the server's byte order and visual into an
    /// RGBA8 [`Frame`]. Handles 8/16/24/32 bits per pixel; anything else is
    /// reported as unsupported rather than guessed.
    fn image_to_rgba(&self, size: Size, depth: u8, visual: u32, data: &[u8]) -> Result<Frame> {
        let format = self.pixmap_format(depth)?;
        let (red_mask, green_mask, blue_mask) = self.visual_masks(visual)?;
        let stride = Self::stride_bytes(size.width, &format);
        let expected = stride * size.height as usize;
        if data.len() < expected {
            return Err(Error::InvalidPixelBuffer { expected, got: data.len() });
        }
        let lsb_first = matches!(
            self.conn.setup().image_byte_order,
            xproto::ImageOrder::LSB_FIRST
        );
        let bpp = format.bits_per_pixel as usize;
        let mut pixels = vec![0u8; size.width as usize * size.height as usize * BYTES_PER_PIXEL];
        for y in 0..size.height as usize {
            let row = &data[y * stride..];
            for x in 0..size.width as usize {
                let raw: u32 = match bpp {
                    32 => {
                        let b = &row[x * 4..x * 4 + 4];
                        if lsb_first {
                            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
                        } else {
                            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
                        }
                    }
                    24 => {
                        let b = &row[x * 3..x * 3 + 3];
                        if lsb_first {
                            u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16
                        } else {
                            u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2])
                        }
                    }
                    16 => {
                        let b = &row[x * 2..x * 2 + 2];
                        if lsb_first {
                            u32::from(u16::from_le_bytes([b[0], b[1]]))
                        } else {
                            u32::from(u16::from_be_bytes([b[0], b[1]]))
                        }
                    }
                    8 => u32::from(row[x]),
                    other => {
                        return Err(Error::Unsupported {
                            what: format!("{other} bits per pixel"),
                        });
                    }
                };
                let offset = (y * size.width as usize + x) * BYTES_PER_PIXEL;
                pixels[offset] = extract_channel(raw, red_mask);
                pixels[offset + 1] = extract_channel(raw, green_mask);
                pixels[offset + 2] = extract_channel(raw, blue_mask);
                pixels[offset + 3] = 0xFF;
            }
        }
        Frame::from_rgba8(size, Scale::new(1.0), pixels)
    }
}

/// Extracts one colour channel from a raw pixel using its visual mask,
/// rescaling the masked field to the full 0..=255 range.
fn extract_channel(raw: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    let value = (raw & mask) >> shift;
    ((value * 255 + max / 2) / max) as u8
}

impl ScreenService for LinuxPlatform {
    fn displays(&self) -> Result<Vec<Display>> {
        match self.randr_displays() {
            Ok(displays) if !displays.is_empty() => Ok(displays),
            _ => Ok(vec![self.root_display()]),
        }
    }

    fn primary(&self) -> Result<Display> {
        self.displays()?.into_iter().find(|d| d.is_primary).ok_or(Error::NoDisplays)
    }
}

impl ScreenCapture for LinuxPlatform {
    /// Captures the part of `rect` that lies inside the desktop (the X root
    /// window covers the whole desktop, so one root grab serves every
    /// display). A rect entirely outside the desktop is `RectOutOfBounds`.
    fn grab(&self, rect: Rect) -> Result<Frame> {
        let bounds = self.root_bounds();
        let clipped = rect
            .intersection(&bounds)
            .ok_or(Error::RectOutOfBounds { requested: rect, bounds })?;
        if self.shm {
            if let Ok(frame) = self.grab_shm(clipped) {
                return Ok(frame);
            }
            // A broken MIT-SHM attempt must not lose the capture: fall back.
        }
        self.grab_get_image(clipped)
    }

    fn grab_display(&self, display_id: u32) -> Result<Frame> {
        let displays = self.displays()?;
        let display = displays
            .iter()
            .find(|d| d.id == display_id)
            .ok_or_else(|| Error::Unsupported { what: format!("unknown display id {display_id}") })?;
        self.grab(display.bounds)
    }
}

impl PermissionService for LinuxPlatform {
    /// X11 has no capture, clipboard, or accessibility permission gate: any
    /// client may read the root window, the clipboard selections, and global
    /// input (XInput2/XRecord) without prompting. [`PermissionState::Granted`]
    /// is the true state of every permission on X11, including Accessibility.
    fn state(&self, _permission: Permission) -> PermissionState {
        PermissionState::Granted
    }

    fn request(&self, _permission: Permission) -> Result<PermissionState> {
        Ok(PermissionState::Granted)
    }
}

impl HotkeyService for LinuxPlatform {
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

impl ClipboardService for LinuxPlatform {
    fn set_image(&self, _frame: &Frame) -> Result<()> {
        Err(unsupported("clipboard", "S7"))
    }

    fn set_text(&self, _text: &str) -> Result<()> {
        Err(unsupported("clipboard", "S7"))
    }
}

impl NotificationService for LinuxPlatform {
    fn notify(&self, _title: &str, _body: &str) -> Result<()> {
        Err(unsupported("notifications", "S7"))
    }
}

impl Fetch for LinuxPlatform {
    fn get(&self, _url: &str) -> Result<Vec<u8>> {
        Err(unsupported("HTTP fetch", "S6"))
    }

    fn put(&self, _url: &str, _body: &[u8], _content_type: &str) -> Result<String> {
        Err(unsupported("HTTP fetch", "S6"))
    }
}

impl ChromeStyle for LinuxPlatform {
    /// GNOME's real chrome values: buttons on the right, 10px corners, a 46px
    /// titlebar, and Ctrl as the primary modifier.
    fn chrome(&self) -> WindowChrome {
        WindowChrome {
            buttons: ButtonSide::Right,
            corner_radius: 10.0,
            titlebar_height: 46.0,
            modifier_label: "Ctrl",
        }
    }

    fn os_name(&self) -> &'static str {
        "linux"
    }
}

impl Paths for LinuxPlatform {
    fn captures_dir(&self) -> PathBuf {
        xdg_or_home("XDG_DOCUMENTS_DIR", "Documents").join("FoxShot")
    }

    fn config_dir(&self) -> PathBuf {
        xdg_or_home("XDG_CONFIG_HOME", ".config").join("foxshot")
    }
}

/// `$variable` when set and non-empty, else `$HOME/fallback`.
fn xdg_or_home(variable: &str, fallback: &str) -> PathBuf {
    if let Some(value) = std::env::var_os(variable).filter(|v| !v.is_empty()) {
        return PathBuf::from(value);
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
    PathBuf::from(home).join(fallback)
}

impl Platform for LinuxPlatform {
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
