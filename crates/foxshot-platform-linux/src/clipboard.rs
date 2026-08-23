//! X11 clipboard: ownership of the CLIPBOARD selection, served from a
//! background thread.
//!
//! X11 clipboards live in the owner process, so every `set_text`/`set_image`
//! spawns a thread that takes ownership of the CLIPBOARD selection and
//! answers `SelectionRequest` events until another client takes the selection
//! over (which arrives as `SelectionClear` and ends the thread). The thread
//! blocks in `wait_for_event` — it never spins. When the process exits the
//! clipboard content goes with it; that is X11 semantics, not a bug.

use foxshot_core::error::{Error, Result};
use foxshot_core::frame::Frame;
use std::sync::mpsc;
use x11rb::CURRENT_TIME;
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, WindowClass,
};
use x11rb::rust_connection::RustConnection;

/// What the clipboard serves after a `set_*` call.
#[derive(Debug)]
pub(super) enum Payload {
    /// UTF-8 text, served as UTF8_STRING, STRING and text/plain variants.
    Text(Vec<u8>),
    /// A PNG-encoded image, served as image/png.
    Png(Vec<u8>),
}

impl Payload {
    fn bytes(&self) -> &[u8] {
        match self {
            Payload::Text(bytes) | Payload::Png(bytes) => bytes,
        }
    }
}

/// Every atom the serving thread needs, interned once.
#[derive(Debug)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    utf8_string: Atom,
    string: Atom,
    text_plain_utf8: Atom,
    text_plain: Atom,
    image_png: Atom,
}

/// Takes ownership of the CLIPBOARD selection and serves `payload` from a
/// fresh background thread. Returns once ownership is established (or the
/// attempt failed), so a caller that exits immediately still knows whether
/// the clipboard took.
pub(super) fn set_payload(payload: Payload) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("foxshot-clipboard".to_string())
        .spawn(move || {
            if let Err(error) = serve(payload, sender) {
                eprintln!("foxshot: clipboard serving failed: {error}");
            }
        })
        .map_err(|error| Error::Transport {
            message: format!("cannot spawn clipboard thread: {error}"),
        })?;
    receiver.recv().map_err(|_| Error::Transport {
        message: "clipboard thread died before taking ownership".to_string(),
    })?
}

/// Encodes a frame as an 8-bit RGBA PNG.
pub(super) fn encode_png(frame: &Frame) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, frame.size().width, frame.size().height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| Error::Transport {
        message: format!("png encoder: {error}"),
    })?;
    writer
        .write_image_data(frame.bytes())
        .map_err(|error| Error::Transport {
            message: format!("png encoder: {error}"),
        })?;
    drop(writer);
    Ok(out)
}

/// The serving thread's body: connect, take ownership, report the outcome,
/// then serve selection requests until the selection is taken away.
fn serve(payload: Payload, sender: mpsc::Sender<Result<()>>) -> Result<()> {
    let (conn, screen_num) = x11rb::connect(None).map_err(|error| Error::Transport {
        message: format!("clipboard thread cannot connect to X server: {error}"),
    })?;
    let setup_result = setup(&conn, screen_num, &payload);
    let (window, atoms) = match setup_result {
        Ok(pair) => {
            let _ = sender.send(Ok(()));
            pair
        }
        Err(error) => {
            let _ = sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    // Blocks on the X event queue; SelectionClear (or a dead connection)
    // ends the loop.
    let outcome = loop {
        match conn.wait_for_event() {
            Ok(Event::SelectionRequest(request)) => answer(&conn, &request, &payload, &atoms),
            // Another client took the selection: our job here is done.
            Ok(Event::SelectionClear(_)) => break Ok(()),
            Ok(_) => {}
            Err(error) => {
                break Err(Error::Transport {
                    message: format!("clipboard event loop failed: {error}"),
                });
            }
        }
    };
    let _ = conn.destroy_window(window);
    outcome
}

/// Creates the owner window, interns atoms, checks the payload fits one
/// request, and takes ownership of CLIPBOARD.
fn setup(conn: &RustConnection, screen_num: usize, payload: &Payload) -> Result<(u32, Atoms)> {
    let atoms = Atoms {
        clipboard: intern(conn, "CLIPBOARD")?,
        targets: intern(conn, "TARGETS")?,
        utf8_string: intern(conn, "UTF8_STRING")?,
        string: u32::from(AtomEnum::STRING),
        text_plain_utf8: intern(conn, "text/plain;charset=utf-8")?,
        text_plain: intern(conn, "text/plain")?,
        image_png: intern(conn, "image/png")?,
    };
    // A property is transferred in one ChangeProperty request, so the payload
    // must fit the server's maximum request size (x11rb enables BIG-REQUESTS
    // at connect time, so this is typically 16 MiB).
    let max = conn.maximum_request_bytes();
    if payload.bytes().len() + 256 > max {
        return Err(Error::Unsupported {
            what: format!(
                "clipboard payload of {} bytes exceeds the server limit of {max} bytes",
                payload.bytes().len()
            ),
        });
    }
    let screen = &conn.setup().roots[screen_num];
    let window = conn.generate_id().map_err(transport)?;
    conn.create_window(
        0,
        window,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::default(),
    )
    .map_err(transport)?
    .check()
    .map_err(transport)?;
    conn.set_selection_owner(window, atoms.clipboard, CURRENT_TIME)
        .map_err(transport)?
        .check()
        .map_err(transport)?;
    let owner = conn
        .get_selection_owner(atoms.clipboard)
        .map_err(transport)?
        .reply()
        .map_err(transport)?
        .owner;
    if owner != window {
        return Err(Error::Transport {
            message: "could not take ownership of the CLIPBOARD selection".to_string(),
        });
    }
    Ok((window, atoms))
}

/// Answers one SelectionRequest: sets the requested property on the
/// requestor (or refuses by answering `None`) and sends SelectionNotify.
fn answer(
    conn: &RustConnection,
    request: &SelectionRequestEvent,
    payload: &Payload,
    atoms: &Atoms,
) {
    // ICCCM: a request with property None uses the obsolete convention of
    // storing onto the target atom.
    let property = if request.property == u32::from(AtomEnum::NONE) {
        request.target
    } else {
        request.property
    };
    let served = serve_target(conn, request, property, payload, atoms);
    if let Err(error) = &served {
        eprintln!("foxshot: clipboard transfer failed: {error}");
    }
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target: request.target,
        property: if served.is_ok() {
            property
        } else {
            u32::from(AtomEnum::NONE)
        },
    };
    let _ = conn.send_event(false, request.requestor, EventMask::NO_EVENT, notify);
    let _ = conn.flush();
}

/// Writes the data for `request`'s target onto `property`, or refuses by
/// returning an error when the target is not one we serve.
fn serve_target(
    conn: &RustConnection,
    request: &SelectionRequestEvent,
    property: Atom,
    payload: &Payload,
    atoms: &Atoms,
) -> Result<()> {
    if request.target == atoms.targets {
        let mut list = vec![atoms.targets];
        match payload {
            Payload::Text(_) => list.extend([
                atoms.utf8_string,
                atoms.string,
                atoms.text_plain_utf8,
                atoms.text_plain,
            ]),
            Payload::Png(_) => list.push(atoms.image_png),
        }
        // Format-32 property data is a run of CARD32s in the connection byte
        // order; x11rb always connects little-endian.
        let mut data = Vec::with_capacity(list.len() * 4);
        for atom in &list {
            data.extend_from_slice(&atom.to_le_bytes());
        }
        conn.change_property(
            PropMode::REPLACE,
            request.requestor,
            property,
            AtomEnum::ATOM,
            32,
            list.len() as u32,
            &data,
        )
        .map_err(transport)?
        .check()
        .map_err(transport)?;
        return Ok(());
    }
    let is_text_target = [
        atoms.utf8_string,
        atoms.string,
        atoms.text_plain_utf8,
        atoms.text_plain,
    ]
    .contains(&request.target);
    let matches = match payload {
        Payload::Text(_) => is_text_target,
        Payload::Png(_) => request.target == atoms.image_png,
    };
    if !matches {
        return Err(Error::Unsupported {
            what: format!("clipboard target atom {}", request.target),
        });
    }
    conn.change_property(
        PropMode::REPLACE,
        request.requestor,
        property,
        request.target,
        8,
        payload.bytes().len() as u32,
        payload.bytes(),
    )
    .map_err(transport)?
    .check()
    .map_err(transport)?;
    Ok(())
}

/// Interns one atom (creating it if the server does not know it yet).
fn intern(conn: &RustConnection, name: &str) -> Result<Atom> {
    conn.intern_atom(false, name.as_bytes())
        .map_err(transport)?
        .reply()
        .map_err(transport)
        .map(|reply| reply.atom)
}

/// Maps any X11 transport failure into Core's error vocabulary.
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport {
        message: error.to_string(),
    }
}
