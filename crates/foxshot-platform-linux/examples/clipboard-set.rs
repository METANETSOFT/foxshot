//! clipboard-set — sets the X11 clipboard and keeps serving it.
//!
//! X11 clipboards live in the owner process, so this tool parks after
//! setting the selection; kill it (or copy something else) to release.
//!
//! Usage:
//!   clipboard-set text <string>
//!   clipboard-set image <width> <height>   (a solid-colour test frame)

use foxshot_core::frame::Frame;
use foxshot_core::geometry::{Scale, Size};
use foxshot_core::platform::ClipboardService as _;
use foxshot_platform_linux::LinuxPlatform;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let platform = match LinuxPlatform::connect() {
        Ok(platform) => platform,
        Err(error) => {
            eprintln!("clipboard-set: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("text") => {
            let text = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "foxshot".to_string());
            platform.set_text(&text)
        }
        Some("image") => {
            let width: u32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(64);
            let height: u32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(48);
            let frame = Frame::new_filled(
                Size { width, height },
                Scale::new(1.0),
                [0x33, 0x66, 0x99, 0xFF],
            );
            platform.set_image(&frame)
        }
        _ => {
            eprintln!("usage: clipboard-set text <string> | clipboard-set image <w> <h>");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => {
            println!("clipboard-set: ownership taken; serving until killed");
            loop {
                std::thread::park();
            }
        }
        Err(error) => {
            eprintln!("clipboard-set: {error}");
            ExitCode::FAILURE
        }
    }
}
