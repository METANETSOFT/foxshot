//! foxshot — the FoxShot command-line application.
//!
//! Slice S2 surface: display enumeration and full/display/region capture to
//! PNG, on Linux/X11. Other operating systems exit non-zero with a message
//! naming the slice that adds them.

use foxshot_core::{Frame, Platform, Rect};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("foxshot: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("foxshot {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("displays") => cmd_displays(),
        Some("capture") => cmd_capture(&args[1..]),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(format!("unknown command '{other}' (try --help)")),
        None => {
            print_usage();
            Err("no command given".to_string())
        }
    }
}

fn print_usage() {
    println!(
        "foxshot {version}\n\
         Usage:\n\
         \x20 foxshot capture --full -o <path>\n\
         \x20 foxshot capture --display <id> -o <path>\n\
         \x20 foxshot capture --region <x>,<y>,<w>,<h> -o <path>\n\
         \x20 foxshot displays\n\
         \x20 foxshot --version",
        version = env!("CARGO_PKG_VERSION")
    );
}

/// Connects the platform adapter of this operating system.
#[cfg(target_os = "linux")]
fn connect() -> Result<Box<dyn Platform>, String> {
    let platform = foxshot_platform_linux::LinuxPlatform::connect().map_err(|e| e.to_string())?;
    Ok(Box::new(platform))
}

/// Every other OS is not built yet — say exactly when it lands.
#[cfg(not(target_os = "linux"))]
fn connect() -> Result<Box<dyn Platform>, String> {
    let slice = match std::env::consts::OS {
        "macos" => "S3",
        "windows" => "S9",
        _ => "not yet scheduled",
    };
    Err(format!("{} is not supported yet (lands in slice {slice})", std::env::consts::OS))
}

fn cmd_displays() -> Result<(), String> {
    let platform = connect()?;
    let displays = platform.screens().displays().map_err(|e| e.to_string())?;
    for display in &displays {
        let primary = if display.is_primary { " primary" } else { "" };
        println!(
            "{id}: {name} {w}x{h}+{x}+{y} scale={scale}{primary}",
            id = display.id,
            name = display.name,
            w = display.bounds.size.width,
            h = display.bounds.size.height,
            x = display.bounds.origin.x,
            y = display.bounds.origin.y,
            scale = display.scale.factor(),
        );
    }
    Ok(())
}

/// What `capture` should grab.
enum CaptureMode {
    /// The primary display.
    Full,
    /// One display by id.
    Display(u32),
    /// An explicit rectangle in desktop coordinates.
    Region(Rect),
}

fn cmd_capture(args: &[String]) -> Result<(), String> {
    let mut mode: Option<CaptureMode> = None;
    let mut output: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut next = |flag: &str| -> Result<&String, String> {
            iter.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--full" => set_mode(&mut mode, CaptureMode::Full)?,
            "--display" => {
                let value = next("--display")?;
                let id: u32 = value
                    .parse()
                    .map_err(|_| format!("--display expects a numeric id, got '{value}'"))?;
                set_mode(&mut mode, CaptureMode::Display(id))?;
            }
            "--region" => {
                let value = next("--region")?;
                set_mode(&mut mode, CaptureMode::Region(parse_region(value)?))?;
            }
            "-o" | "--output" => output = Some(PathBuf::from(next("-o")?)),
            other => return Err(format!("unknown capture option '{other}'")),
        }
    }
    let mode = mode.ok_or("capture needs one of --full, --display <id>, --region <x>,<y>,<w>,<h>")?;
    let output = output.ok_or("capture needs -o <path>")?;

    let platform = connect()?;
    let frame = match mode {
        CaptureMode::Full => {
            let primary = platform.screens().primary().map_err(|e| e.to_string())?;
            platform.capture().grab_display(primary.id).map_err(|e| e.to_string())?
        }
        CaptureMode::Display(id) => {
            platform.capture().grab_display(id).map_err(|e| e.to_string())?
        }
        CaptureMode::Region(rect) => platform.capture().grab(rect).map_err(|e| e.to_string())?,
    };
    write_png(&output, &frame)?;
    println!(
        "captured {w}x{h} -> {path}",
        w = frame.size().width,
        h = frame.size().height,
        path = output.display()
    );
    Ok(())
}

fn set_mode(mode: &mut Option<CaptureMode>, value: CaptureMode) -> Result<(), String> {
    if mode.replace(value).is_some() {
        return Err("use only one of --full, --display, --region".to_string());
    }
    Ok(())
}

/// Parses `<x>,<y>,<w>,<h>` into a [`Rect`].
fn parse_region(value: &str) -> Result<Rect, String> {
    let bad = || format!("--region expects <x>,<y>,<w>,<h>, got '{value}'");
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 4 {
        return Err(bad());
    }
    let x: i32 = parts[0].trim().parse().map_err(|_| bad())?;
    let y: i32 = parts[1].trim().parse().map_err(|_| bad())?;
    let width: u32 = parts[2].trim().parse().map_err(|_| bad())?;
    let height: u32 = parts[3].trim().parse().map_err(|_| bad())?;
    Ok(Rect::from_xywh(x, y, width, height))
}

/// Encodes a frame as 8-bit RGBA PNG.
fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    let file = std::fs::File::create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut encoder =
        png::Encoder::new(std::io::BufWriter::new(file), frame.size().width, frame.size().height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| format!("png encoder: {e}"))?;
    writer.write_image_data(frame.bytes()).map_err(|e| format!("png encoder: {e}"))
}
