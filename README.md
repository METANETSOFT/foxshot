# FoxShot

A screen capture, annotation and sharing tool for macOS, Linux and Windows, written in Rust.

> **Pre-alpha.** Screen capture works on Linux today and the update check is live. Everything else is in progress. The build order is in `docs/diagrams`.

## What works today

- Capture the full screen, a single display, or a pixel region on Linux via X11 — XShm where the extension is present, `GetImage` as the fallback
- List displays with their geometry, scale and which one is primary
- PNG output
- Check for component updates against the published manifest over a real HTTPS request

```
foxshot displays
foxshot capture --full -o shot.png
foxshot capture --display 0 -o shot.png
foxshot capture --region 100,100,800,600 -o shot.png
foxshot update --check
```

## What it will do

- Region, window, full-screen and scrolling capture
- Screen recording and GIF
- Annotation with 17 mark types layered over the capture, never touching the pixels underneath
- Upload to Cloudflare R2, Amazon S3 and free hosts, with the link placed on the clipboard
- OCR, QR and a colour picker

## Architecture

One Core, three platform adapters.

Core owns all behaviour and contains no `cfg(target_os)` in its code; everything platform-specific sits behind a trait in `core::platform`. A change in Core reaches all three platforms at once. Core performs no I/O at all — even HTTP goes out through the adapter's `Fetch` trait, which is why `UpdateChecker::compare` is a pure function over a manifest string.

Core, each adapter and each feature module carry their own version and update independently. At startup `updates.json` is read and anything newer is reported. A manifest entry with a null download is reportable but not installable, which is a real state rather than a missing value.

## Repository layout

```
crates/foxshot-core              Core: capture pipeline, annotation model,
                                 module registry, update checker, platform traits
crates/foxshot-platform-linux    X11 adapter: RandR, XShm, real HTTP
crates/foxshot-platform-macos    CoreGraphics adapter, not yet run on hardware
crates/foxshot-platform-windows  placeholder until its slice
crates/foxshot-app               the foxshot command line binary
docs/diagrams                    architecture, flows and the build order
design/foxshot.html              clickable design prototype, 157 scenarios
updates.json                     the published version manifest
```

## Build

```
cargo build --workspace
cargo test --workspace
```

The Linux adapter needs an X server. Capture can be verified headlessly:

```
xvfb-run -a --server-args="-screen 0 800x600x24" ./target/debug/foxshot capture --full -o shot.png
```

## Documents

- `PRODUCT.md` — product truth, users, scope
- `DESIGN.md` — the visual system
- `design/foxshot.html` — clickable design prototype, 157 scenarios
- `FACTORY.md` — how the work is run

## License

PolyForm Noncommercial 1.0.0, full text in `LICENSE.md`. You may read, modify and share it; commercial rights are reserved to Metanetsoft. Plain-language summary in `LICENSING.md`.
