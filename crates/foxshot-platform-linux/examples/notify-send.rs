//! notify-send — posts one desktop notification and prints the result.
//!
//! Usage: notify-send [title] [body]

use foxshot_core::platform::NotificationService as _;
use foxshot_platform_linux::LinuxPlatform;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let title = args.first().map(String::as_str).unwrap_or("FoxShot");
    let body = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("notification test");
    let platform = match LinuxPlatform::connect() {
        Ok(platform) => platform,
        Err(error) => {
            eprintln!("notify-send: {error}");
            return ExitCode::FAILURE;
        }
    };
    match platform.notify(title, body) {
        Ok(id) => {
            println!("notify-send: notification id {id}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("notify-send: {error}");
            ExitCode::FAILURE
        }
    }
}
