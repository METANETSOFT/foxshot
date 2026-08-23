//! foxshot — the FoxShot command-line application.
//!
//! Slice S2 surface: display enumeration and full/display/region capture to
//! PNG, on Linux/X11; slice S3 adds the macOS adapter. Slice S8 adds
//! `update --check`: fetch the published update manifest through the
//! platform's `Fetch` and report what would update. Slice S6 adds
//! `upload <file> [--target r2|s3|free]` and `capture --upload`: sends a PNG
//! to Cloudflare R2, Amazon S3 or an anonymous free host, with credentials
//! read from the environment only. Other operating systems exit non-zero
//! with a message naming the slice that adds them.

use foxshot_core::{Credentials, Frame, FreeHostTarget, ModuleRegistry, Platform, Rect, S3Target,
    UpdateChecker, UpdateManifest, UpdateReport, UpdateStatus, UploadTarget};
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
        Some("upload") => cmd_upload(&args[1..]),
        Some("update") => cmd_update(&args[1..]),
        Some("daemon") => cmd_daemon(&args[1..]),
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
         \x20 foxshot capture --full -o <path> [--upload [--target r2|s3|free]]\n\
         \x20 foxshot capture --display <id> -o <path>\n\
         \x20 foxshot capture --region <x>,<y>,<w>,<h> -o <path>\n\
         \x20 foxshot upload <file> [--target r2|s3|free]\n\
         \x20 foxshot daemon [--full-key <accel>] [--region-key <accel>] [--upload]\n\
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
    let mut upload = false;
    let mut target_name = "r2".to_string();
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
            "--upload" => upload = true,
            "--target" => target_name = next("--target")?.clone(),
            other => return Err(format!("unknown capture option '{other}'")),
        }
    }
    let mode = mode.ok_or("capture needs one of --full, --display <id>, --region <x>,<y>,<w>,<h>")?;
    if output.is_none() && !upload {
        return Err("capture needs -o <path> (or --upload to send it straight to a target)".into());
    }

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
    if let Some(path) = &output {
        write_png(path, &frame)?;
        println!(
            "captured {w}x{h} -> {path}",
            w = frame.size().width,
            h = frame.size().height,
            path = path.display()
        );
    }
    if upload {
        let key = output
            .as_ref()
            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .unwrap_or_else(capture_key);
        let url = upload_bytes(platform.as_ref(), encode_png(&frame)?, &key, &target_name)?;
        println!("uploaded {key} -> {url}");
    }
    Ok(())
}

/// Object key for a capture without `-o`: `capture-<unix-seconds>.png`.
fn capture_key() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    format!("capture-{secs}.png")
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

/// Encodes a frame as 8-bit RGBA PNG into a fresh buffer.
fn encode_png(frame: &Frame) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder =
        png::Encoder::new(&mut out, frame.size().width, frame.size().height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| format!("png encoder: {e}"))?;
    writer.write_image_data(frame.bytes()).map_err(|e| format!("png encoder: {e}"))?;
    drop(writer);
    Ok(out)
}

/// Writes a frame as an 8-bit RGBA PNG file.
fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    std::fs::write(path, encode_png(frame)?)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Uploads `bytes` under `key` to the named target, after validating the
/// target through the platform's `Fetch`. Returns the resulting URL.
fn upload_bytes(
    platform: &dyn Platform,
    bytes: Vec<u8>,
    key: &str,
    target_name: &str,
) -> Result<String, String> {
    let target = build_target(target_name)?;
    target.validate(platform.fetch()).map_err(|e| e.to_string())?;
    target.upload(platform.fetch(), &bytes, key).map_err(|e| e.to_string())
}

fn cmd_upload(args: &[String]) -> Result<(), String> {
    let mut file: Option<PathBuf> = None;
    let mut target_name = "r2".to_string();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                target_name = iter
                    .next()
                    .ok_or("--target needs a value (r2, s3 or free)")?
                    .clone();
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown upload option '{other}'"));
            }
            other => {
                if file.is_some() {
                    return Err(format!("unexpected extra argument '{other}'"));
                }
                file = Some(PathBuf::from(other));
            }
        }
    }
    let file = file.ok_or("upload needs a file: foxshot upload <file> [--target r2|s3|free]")?;
    let platform = connect()?;
    let bytes =
        std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let key = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("cannot derive an object key from {}", file.display()))?;
    let url = upload_bytes(platform.as_ref(), bytes, &key, &target_name)?;
    println!("uploaded {key} -> {url}");
    Ok(())
}

/// Default accelerator for a full-desktop capture (macOS's Cmd+Shift+3,
/// with Ctrl because X11 desktops reserve Super for the window manager).
const DEFAULT_FULL_KEY: &str = "Ctrl+Shift+3";
/// Default accelerator for a region capture.
const DEFAULT_REGION_KEY: &str = "Ctrl+Shift+4";

/// Linux daemon: binds the capture keys globally, then serves captures in a
/// loop. Each fire captures, saves a PNG under the captures dir, copies it
/// to the clipboard, posts a notification, and (with `--upload`) uploads.
/// The region key captures the full desktop for now — interactive region
/// selection lands with the GUI overlay slice — and the daemon says so.
#[cfg(target_os = "linux")]
fn cmd_daemon(args: &[String]) -> Result<(), String> {
    let mut full_key = DEFAULT_FULL_KEY.to_string();
    let mut region_key = DEFAULT_REGION_KEY.to_string();
    let mut upload = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut next = |flag: &str| -> Result<&String, String> {
            iter.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--full-key" => full_key = next("--full-key")?.clone(),
            "--region-key" => region_key = next("--region-key")?.clone(),
            "--upload" => upload = true,
            other => return Err(format!("unknown daemon option '{other}'")),
        }
    }

    let platform =
        foxshot_platform_linux::LinuxPlatform::connect().map_err(|e| e.to_string())?;
    use foxshot_core::platform::{HotkeyService, Paths};
    HotkeyService::register(&platform, "full", &full_key).map_err(|e| e.to_string())?;
    HotkeyService::register(&platform, "region", &region_key).map_err(|e| e.to_string())?;
    println!(
        "foxshot daemon: '{full_key}' captures the desktop, '{region_key}' too \
         (interactive region selection lands with the GUI slice); \
         clipboard + notifications on, upload {}",
        if upload { "on" } else { "off" }
    );
    let dir = Paths::captures_dir(&platform);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    loop {
        match platform.poll_hotkey(std::time::Duration::from_secs(3600)) {
            Ok(Some(id)) => daemon_capture(&platform, &dir, &id, upload),
            Ok(None) => {}
            Err(error) => eprintln!("foxshot daemon: hotkey poll failed: {error}"),
        }
    }
}

/// One daemon capture: grab the desktop, save, copy, notify, maybe upload.
/// Failures are printed, not fatal — the daemon keeps serving the keys.
#[cfg(target_os = "linux")]
fn daemon_capture(
    platform: &foxshot_platform_linux::LinuxPlatform,
    dir: &Path,
    id: &str,
    upload: bool,
) {
    use foxshot_core::platform::{ClipboardService, NotificationService, ScreenCapture};
    let result = (|| -> Result<(), String> {
        let frame = ScreenCapture::grab(platform, full_bounds(platform)?)
            .map_err(|e| e.to_string())?;
        let path = dir.join(capture_key());
        write_png(&path, &frame)?;
        println!(
            "captured {w}x{h} ({id}) -> {path}",
            w = frame.size().width,
            h = frame.size().height,
            path = path.display()
        );
        ClipboardService::set_image(platform, &frame).map_err(|e| e.to_string())?;
        if upload {
            let key = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(capture_key);
            let url = upload_bytes(platform, encode_png(&frame)?, &key, "r2")?;
            println!("uploaded {key} -> {url}");
        }
        let body = match upload {
            true => "Screenshot saved, copied to the clipboard and uploaded",
            false => "Screenshot saved and copied to the clipboard",
        };
        match NotificationService::notify(platform, "FoxShot", body) {
            Ok(nid) => println!("notified (id {nid}): {body}"),
            Err(error) => eprintln!("foxshot daemon: notification failed: {error}"),
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("foxshot daemon: capture failed: {error}");
    }
}

/// The whole desktop bounds, via the primary display.
#[cfg(target_os = "linux")]
fn full_bounds(platform: &foxshot_platform_linux::LinuxPlatform) -> Result<Rect, String> {
    use foxshot_core::platform::ScreenService;
    Ok(ScreenService::primary(platform).map_err(|e| e.to_string())?.bounds)
}

/// Other OSes have no global-hotkey adapter yet.
#[cfg(not(target_os = "linux"))]
fn cmd_daemon(_args: &[String]) -> Result<(), String> {
    Err("daemon needs the Linux/X11 adapter (other platforms land in later slices)".to_string())
}

/// Default endpoint of the anonymous `--target free` host.
const DEFAULT_FREE_ENDPOINT: &str = "https://transfer.sh";

/// Builds the upload target named on the command line.
///
/// Credentials come from the process environment **only** — never a file,
/// never a flag (a flag would land in shell history). When a required
/// variable is missing the error names exactly which variables the target
/// needs; values that were read are never printed.
fn build_target(name: &str) -> Result<Box<dyn UploadTarget>, String> {
    match name {
        "r2" => {
            let values = require_env(&[
                "FOXSHOT_R2_ACCOUNT_ID",
                "FOXSHOT_R2_BUCKET",
                "FOXSHOT_R2_ACCESS_KEY_ID",
                "FOXSHOT_R2_SECRET_ACCESS_KEY",
            ])?;
            let creds = Credentials::new(values[2].clone(), values[3].clone());
            let mut target = S3Target::r2(&values[0], &values[1], creds);
            if let Some(base) = env_value("FOXSHOT_R2_PUBLIC_BASE") {
                target = target.with_public_base(base);
            }
            Ok(Box::new(target))
        }
        "s3" => {
            let values = require_env(&[
                "FOXSHOT_S3_REGION",
                "FOXSHOT_S3_BUCKET",
                "FOXSHOT_S3_ACCESS_KEY_ID",
                "FOXSHOT_S3_SECRET_ACCESS_KEY",
            ])?;
            let creds = Credentials::new(values[2].clone(), values[3].clone());
            Ok(Box::new(S3Target::aws(&values[0], &values[1], creds)))
        }
        "free" => Ok(Box::new(FreeHostTarget::new(
            env_value("FOXSHOT_FREE_ENDPOINT").unwrap_or_else(|| DEFAULT_FREE_ENDPOINT.to_string()),
        ))),
        other => Err(format!("unknown upload target '{other}' (expected r2, s3 or free)")),
    }
}

/// An environment variable's value when set and non-empty.
fn env_value(name: &str) -> Option<String> {
    std::env::var_os(name).filter(|value| !value.is_empty()).map(|value| {
        value.to_str().map(str::to_string).unwrap_or_default()
    })
}

/// Reads every variable in `names`. Fails listing exactly which ones are
/// missing — names only, never values.
fn require_env(names: &[&str]) -> Result<Vec<String>, String> {
    let missing: Vec<&str> =
        names.iter().copied().filter(|name| env_value(name).is_none()).collect();
    if !missing.is_empty() {
        return Err(format!(
            "missing environment variables: {}\n\
             set them in the environment and retry (values are read from the \
             environment only and are never logged)",
            missing.join(", ")
        ));
    }
    Ok(names.iter().map(|name| env_value(name).unwrap_or_default()).collect())
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
