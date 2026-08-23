//! X11 global hotkeys: `XGrabKey` on the root window.
//!
//! Accelerators like `"Ctrl+Shift+4"` are parsed into a keysym plus a
//! modifier mask, the keysym is mapped to a keycode through the server's
//! keyboard mapping, and the key is grabbed on the root window four times:
//! with and without Lock (Caps Lock) and Mod2 (Num Lock), so neither lock
//! key breaks the binding. Passive grabs are global — they fire while any
//! other application has focus, which is why [`HotkeyService::is_global`]
//! reports `true`.
//!
//! [`HotkeyService::is_global`]: foxshot_core::platform::HotkeyService::is_global

use foxshot_core::error::{Error, Result};
use std::os::unix::io::AsRawFd as _;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt as _, EventMask, GrabMode, ModMask,
};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;

// Modifier mask bits (the XCB ModMask values).
/// Shift.
const MOD_SHIFT: u16 = 1;
/// Caps Lock.
const MOD_LOCK: u16 = 2;
/// Control.
const MOD_CONTROL: u16 = 4;
/// Alt (Mod1).
const MOD_ALT: u16 = 8;
/// Num Lock (Mod2).
const MOD_MOD2: u16 = 16;
/// Super/Windows (Mod4).
const MOD_SUPER: u16 = 64;

/// The lock variants a binding is grabbed under, so Caps Lock and Num Lock
/// do not change whether the combination fires.
const LOCK_VARIANTS: [u16; 4] = [0, MOD_LOCK, MOD_MOD2, MOD_LOCK | MOD_MOD2];

/// A registered binding: what the server reports in the KeyPress event.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Grab {
    /// The grabbed keycode.
    keycode: u8,
    /// The modifier mask without any lock variants.
    mods: u16,
}

/// Parses `"Ctrl+Shift+4"` into `(keysym, modifier mask)`.
///
/// The key is either a single printable ASCII character (letters are taken
/// lowercase — Shift belongs in the mask, not the keysym) or a name from a
/// small table (F1–F12, Escape, space, arrows, ...). Anything else is
/// reported as unsupported rather than guessed.
pub(crate) fn parse_accelerator(accelerator: &str) -> Result<(u32, u16)> {
    let mut mods: u16 = 0;
    let mut parts = accelerator.split('+').map(str::trim).peekable();
    let mut key: Option<&str> = None;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            key = Some(part);
            break;
        }
        mods |= match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "shift" => MOD_SHIFT,
            "alt" | "mod1" => MOD_ALT,
            "super" | "meta" | "cmd" | "win" | "mod4" => MOD_SUPER,
            other => {
                return Err(Error::Unsupported {
                    what: format!("modifier '{other}' in accelerator '{accelerator}'"),
                });
            }
        };
    }
    let key = key.ok_or_else(|| Error::Unsupported {
        what: format!("accelerator '{accelerator}' has no key"),
    })?;
    if key.is_empty() {
        return Err(Error::Unsupported {
            what: format!("accelerator '{accelerator}' has an empty key"),
        });
    }
    Ok((keysym_of(key)?, mods))
}

/// The keysym for one key name: ASCII printable characters map to their
/// code point; everything else comes from a name table.
fn keysym_of(key: &str) -> Result<u32> {
    let lowered = key.to_ascii_lowercase();
    let mut chars = lowered.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        if only.is_ascii() && !only.is_ascii_control() {
            return Ok(u32::from(only as u8));
        }
    }
    let keysym = match lowered.as_str() {
        "escape" | "esc" => 0xFF1B,
        "return" | "enter" => 0xFF0D,
        "tab" => 0xFF09,
        "backspace" => 0xFF08,
        "delete" | "del" => 0xFFFF,
        "insert" | "ins" => 0xFF63,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "pageup" | "prior" => 0xFF55,
        "pagedown" | "next" => 0xFF56,
        "left" => 0xFF51,
        "up" => 0xFF52,
        "right" => 0xFF53,
        "down" => 0xFF54,
        "print" | "printscreen" => 0xFF61,
        "scrolllock" => 0xFF14,
        "pause" => 0xFF13,
        other => {
            if let Some(number) = other
                .strip_prefix('f')
                .and_then(|digits| digits.parse::<u32>().ok())
                .filter(|n| (1..=35).contains(n))
            {
                0xFFBD + number // F1 is 0xFFBE
            } else {
                return Err(Error::Unsupported { what: format!("key '{key}'") });
            }
        }
    };
    Ok(keysym)
}

/// Maps a keysym to a keycode through the server's keyboard mapping.
/// Letters are looked up case-insensitively: the mapping stores the
/// lowercase keysym at level 0 and the uppercase one at level 1.
fn keycode_of(conn: &RustConnection, keysym: u32) -> Result<u8> {
    let setup = conn.setup();
    let first = setup.min_keycode;
    let count = setup.max_keycode - first + 1;
    let mapping = conn
        .get_keyboard_mapping(first, count)
        .map_err(transport)?
        .reply()
        .map_err(transport)?;
    let per = mapping.keysyms_per_keycode as usize;
    let byte = u8::try_from(keysym).unwrap_or(0);
    let lowered = u32::from(byte.to_ascii_lowercase());
    let uppered = u32::from(byte.to_ascii_uppercase());
    let wanted: &[u32] = if byte.is_ascii_alphabetic() {
        &[lowered, uppered]
    } else {
        &[keysym]
    };
    for (index, chunk) in mapping.keysyms.chunks(per).enumerate() {
        if chunk.iter().any(|candidate| wanted.contains(candidate)) {
            return Ok(first + index as u8);
        }
    }
    Err(Error::Unsupported {
        what: format!("keysym {keysym:#x} has no keycode in the keyboard mapping"),
    })
}

/// Grabs `accelerator` for `id` on the root window, under every lock
/// variant. A combination another client already holds is reported
/// clearly, and any partial grabs are rolled back.
pub(crate) fn register(
    conn: &RustConnection,
    screen_num: usize,
    id: &str,
    accelerator: &str,
) -> Result<Grab> {
    let (keysym, mods) = parse_accelerator(accelerator)?;
    let keycode = keycode_of(conn, keysym)?;
    let root = conn.setup().roots[screen_num].root;
    // Passive grab events are delivered to clients that selected KeyPress on
    // the grab window; other clients' selections on the root are untouched.
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::KEY_PRESS),
    )
    .map_err(transport)?
    .check()
    .map_err(transport)?;
    for (done, variant) in LOCK_VARIANTS.into_iter().enumerate() {
        let outcome = conn
            .grab_key(
                false,
                root,
                ModMask::from(mods | variant),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(transport)?
            .check();
        if let Err(error) = outcome {
            for rollback in &LOCK_VARIANTS[..done] {
                let _ = conn.ungrab_key(keycode, root, ModMask::from(mods | rollback));
            }
            let message = if is_already_grabbed(&error) {
                format!(
                    "hotkey '{accelerator}' (id '{id}') is already grabbed by another client"
                )
            } else {
                format!("grabbing hotkey '{accelerator}' (id '{id}') failed: {error}")
            };
            return Err(Error::Transport { message });
        }
    }
    Ok(Grab { keycode, mods })
}

/// Removes a binding previously made by [`register`].
pub(crate) fn unregister(conn: &RustConnection, screen_num: usize, grab: Grab) -> Result<()> {
    let root = conn.setup().roots[screen_num].root;
    for variant in LOCK_VARIANTS {
        conn.ungrab_key(grab.keycode, root, ModMask::from(grab.mods | variant))
            .map_err(transport)?
            .check()
            .map_err(transport)?;
    }
    Ok(())
}

/// Whether an X error is the "already grabbed" condition (BadAccess from a
/// GrabKey request).
fn is_already_grabbed(error: &x11rb::errors::ReplyError) -> bool {
    matches!(
        error,
        x11rb::errors::ReplyError::X11Error(x11_error)
            if x11_error.error_kind == ErrorKind::Access
    )
}

/// Waits up to `timeout` for one of the registered hotkeys to fire and
/// returns its id. Blocks on the connection's file descriptor via `poll(2)`
/// — no spinning. KeyPress events unrelated to a registration are consumed
/// and ignored.
pub(crate) fn poll(
    conn: &RustConnection,
    registrations: &std::collections::HashMap<String, Grab>,
    timeout: Duration,
) -> Result<Option<String>> {
    let deadline = Instant::now() + timeout;
    loop {
        while let Some(event) = conn.poll_for_event().map_err(transport)? {
            if let Event::KeyPress(press) = event {
                let mods = u16::from(press.state) & !(MOD_LOCK | MOD_MOD2);
                for (id, grab) in registrations {
                    if press.detail == grab.keycode && mods == grab.mods {
                        return Ok(Some(id.clone()));
                    }
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let mut fds = [libc::pollfd {
            fd: conn.stream().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        let millis = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        // Safety: `fds` points at one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, millis) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue; // a signal interrupted the wait: honour the deadline
            }
            return Err(Error::Transport { message: format!("poll on X connection failed: {error}") });
        }
        if ready == 0 {
            return Ok(None);
        }
    }
}

/// Maps any X11 transport failure into Core's error vocabulary.
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport { message: error.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(accelerator: &str) -> (u32, u16) {
        parse_accelerator(accelerator).expect("accelerator parses")
    }

    #[test]
    fn ctrl_shift_digit() {
        assert_eq!(parse("Ctrl+Shift+4"), (0x34, MOD_CONTROL | MOD_SHIFT));
    }

    #[test]
    fn ctrl_alt_letter_is_lowercased() {
        assert_eq!(parse("Ctrl+Alt+R"), (u32::from(b'r'), MOD_CONTROL | MOD_ALT));
    }

    #[test]
    fn bare_key_and_synonyms() {
        assert_eq!(parse("F5"), (0xFFC2, 0));
        assert_eq!(parse("Control+Escape"), (0xFF1B, MOD_CONTROL));
        assert_eq!(parse("Super+Space"), (0x20, MOD_SUPER));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_accelerator("Ctrl+").is_err());
        assert!(parse_accelerator("").is_err());
        assert!(parse_accelerator("Hyper+X").is_err());
        assert!(parse_accelerator("Ctrl+NoSuchKey").is_err());
    }
}
