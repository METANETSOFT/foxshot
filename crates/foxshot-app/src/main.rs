//! foxshot — the FoxShot command-line application.
//!
//! Slice S2 surface: display enumeration and full/display/region capture to
//! PNG, on Linux/X11; slice S3 adds the macOS adapter. Slice S8 adds
//! `update --check`: fetch the published update manifest through the
//! platform's `Fetch` and report what would update. Other operating systems
//! exit non-zero with a message naming the slice that adds them.

use foxshot_core::{Frame, ModuleRegistry, Platform, Rect, UpdateChecker, UpdateManifest,
    UpdateReport, UpdateStatus};
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
        Some("update") => cmd_update(&args[1..]),
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
         \x20 foxshot update --check\n\
         \x20 foxshot --version\n\
         \x20 foxshot --help",
        version = env!("CARGO_PKG_VERSION")
    );
}

/// Connects the platform adapter of this operating system.
#[cfg(target_os = "linux")]
fn connect() -> Result<Box<dyn Platform>, String> {
    let platform = foxshot_platform_linux::LinuxPlatform::connect().map_err(|e| e.to_string())?;
    Ok(Box::new(platform))
}

/// Connects the platform adapter of this operating system.
#[cfg(target_os = "macos")]
fn connect() -> Result<Box<dyn Platform>, String> {
    let platform = foxshot_platform_macos::MacosPlatform::connect().map_err(|e| e.to_string())?;
    Ok(Box::new(platform))
}

/// Every other OS is not built yet — say exactly when it lands.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn connect() -> Result<Box<dyn Platform>, String> {
    let slice = match std::env::consts::OS {
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

/// Where the FoxShot project publishes its update manifest.
const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/METANETSOFT/foxshot/main/updates.json";

fn cmd_update(args: &[String]) -> Result<(), String> {
    match args {
        [flag] if flag == "--check" => cmd_update_check(),
        [] => Err("update needs --check".to_string()),
        other => Err(format!("unknown update option '{}'", other.join(" "))),
    }
}

/// Fetches the published manifest through the platform's `Fetch` trait,
/// compares it against what this build contains, and prints the report.
/// Updates found still exit successfully — only a real failure (no network,
/// malformed manifest) exits non-zero.
fn cmd_update_check() -> Result<(), String> {
    let registry = build_registry();
    let platform = connect()?;
    let bytes = platform.fetch().get(MANIFEST_URL).map_err(|e| e.to_string())?;
    let json =
        String::from_utf8(bytes).map_err(|e| format!("update manifest is not UTF-8: {e}"))?;
    let manifest = UpdateManifest::from_json(&json).map_err(|e| e.to_string())?;
    let report = UpdateChecker::compare(&registry, &manifest);
    print_report(&report);
    Ok(())
}

/// The registry of what this build actually contains: Core at its own
/// version plus the adapter of this OS at its version. No feature modules
/// ship in this build, so none are registered.
fn build_registry() -> ModuleRegistry {
    build_registry_for_os(ModuleRegistry::new())
}

/// Registers the Linux adapter at its crate version.
#[cfg(target_os = "linux")]
fn build_registry_for_os(registry: ModuleRegistry) -> ModuleRegistry {
    registry.with_installed(
        foxshot_core::Component::Adapter("linux".to_string()),
        foxshot_platform_linux::VERSION.parse().expect("adapter version is valid"),
    )
}

/// Registers the macOS adapter at its crate version.
#[cfg(target_os = "macos")]
fn build_registry_for_os(registry: ModuleRegistry) -> ModuleRegistry {
    registry.with_installed(
        foxshot_core::Component::Adapter("macos".to_string()),
        foxshot_platform_macos::VERSION.parse().expect("adapter version is valid"),
    )
}

/// Other OSes have no adapter crate to register yet.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn build_registry_for_os(registry: ModuleRegistry) -> ModuleRegistry {
    registry
}

fn print_report(report: &UpdateReport) {
    println!("core: {}", describe(&report.core));
    for (component, status) in &report.per_component {
        println!("{component}: {}", describe(status));
    }
    println!(
        "restart required to apply updates: {}",
        if report.requires_restart() { "yes" } else { "no" }
    );
}

/// One human-readable status phrase for one component.
fn describe(status: &UpdateStatus) -> String {
    match status {
        UpdateStatus::UpToDate => "up to date".to_string(),
        UpdateStatus::Available { from, to, installable } => {
            let kind = if *installable { "installable" } else { "not installable" };
            format!("update {from} -> {to} available ({kind})")
        }
        UpdateStatus::BlockedByCore { needs, have } => {
            format!("update blocked by core (needs core {needs}, have {have})")
        }
        UpdateStatus::NotInstalled { available, installable } => {
            let package = if *installable { "package published" } else { "no package published yet" };
            format!("not installed ({available} available, {package})")
        }
    }
}
