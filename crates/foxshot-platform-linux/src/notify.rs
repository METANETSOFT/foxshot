//! Desktop notifications over the DBus session bus
//! (`org.freedesktop.Notifications.Notify`).
//!
//! When there is no session bus at all, [`notify`] reports
//! [`Error::Unsupported`] saying exactly that — an honest answer about the
//! environment, not a stub. When a bus exists but the call fails (no
//! notification daemon, malformed reply), that is a transport error.

use foxshot_core::error::{Error, Result};
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::Value;

/// How long the notification stays on screen, in milliseconds.
const EXPIRE_TIMEOUT_MS: i32 = 5000;

/// Shows a notification and returns the id the notification daemon assigned
/// to it.
pub(super) fn notify(title: &str, body: &str) -> Result<u32> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS")
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(Error::Unsupported {
            what: "desktop notifications: the DBus session bus is absent \
                   (DBUS_SESSION_BUS_ADDRESS is not set)"
                .to_string(),
        });
    }
    let connection = zbus::blocking::connection::Builder::session()
        .map_err(|error| Error::Transport {
            message: format!("invalid session bus address: {error}"),
        })?
        .method_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| Error::Transport {
            message: format!("cannot connect to the DBus session bus: {error}"),
        })?;
    // Notify(s app_name, u replaces_id, s app_icon, s summary, s body,
    //        as actions, a{sv} hints, i expire_timeout) -> u id
    let message = connection
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "foxshot",
                0u32,
                "",
                title,
                body,
                Vec::<String>::new(),
                HashMap::<&str, Value>::new(),
                EXPIRE_TIMEOUT_MS,
            ),
        )
        .map_err(|error| Error::Transport {
            message: format!("org.freedesktop.Notifications.Notify failed: {error}"),
        })?;
    message
        .body()
        .deserialize::<u32>()
        .map_err(|error| Error::Transport {
            message: format!("malformed Notify reply: {error}"),
        })
}
