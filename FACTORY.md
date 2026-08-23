# FoxShot — Factory Charter

## What this is
A native screen-capture tool for macOS, Linux and Windows, written in Rust as a
monorepo: one object-oriented Core that owns all behaviour, and three thin platform
adapters bound to it for both appearance and compatibility. A change in Core reaches
all three at once. Core, each adapter and each feature module carry their own version
and update independently.

## Product truth
Lives in PRODUCT.md. Visual system lives in DESIGN.md. The clickable design prototype
is design/foxshot.html (157 scenarios). Do not re-derive any of these; read them.

## Non-negotiables
- Rust. wgpu + a custom widget layer for UI. No Electron, no webview.
- Dependency direction is one-way: adapters and modules depend on Core; Core depends on
  nothing. Core never has a cfg(target_os) branch.
- Per-crate versions. Nothing may unify them.
- Dark theme only. Roboto. The action colour appears only on actionable elements.
- No code derived from ShareX. Clean-room; behaviour parity is the target, not source.
- v1 ships capture + editor + upload. Video, OCR, GIF, QR are designed and deferred.

## Vocabulary — one name per thing
- **Core** — the crate foxshot-core. Never "engine", never "backend".
- **Adapter** — a platform crate (platform-macos/linux/windows). Never "driver", "port".
- **Module** — an independently versioned feature crate. Never "plugin", never "addon".
- **Frame** — one captured RGBA buffer plus its scale factor. Never "image", "bitmap".
- **Mark** — one annotation object on a Frame. Never "shape", never "layer".
- **Finding** — the readable row a Mark writes into the editor's side list.
- **Target** — an upload destination (R2, S3, a free host). Never "provider", "backend".
- **Manifest** — updates.json in the GitHub repo. Never "feed", never "index".

## How work runs
Vertical slices only, in the order set by docs/diagrams/04-insa-adimlari.drawio.
Each slice ends with a mechanically checkable command, and no slice is called done
without that command's real output.

## Executor
kole-kimi (MCP). Claude writes the briefs, decides, and judges evidence; Kimi does all
labor. A report is not evidence — the diff and the command output are.
