# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop

> Schema v1 enumerates `web|ios|android|adaptive`; none describes a native desktop
> application. Recorded as `desktop` deliberately: macOS primary, Linux second,
> Windows later. `ios.md` / `android.md` do not apply to this product.

## Stack

**Monorepo, one Core, three platform targets.** Rust throughout, organised as an
object-oriented Core class system with three thin platform layers bound to it:

- `core/` — the class hierarchy that owns all product behaviour: capture pipeline,
  annotation model, upload, library, module registry, settings. Platform-agnostic.
- `platform/macos/`, `platform/linux/`, `platform/windows/` — adapters that bind to
  Core for **both appearance and compatibility**. Each implements the same Core-declared
  interfaces (window chrome, permissions, hotkeys, clipboard, screen enumeration).
- A change in Core propagates to all three at once; the three are developed and reviewed
  as equals, never one primary and two ports.

UI layer: **wgpu** (Metal on macOS, Vulkan on Linux, DX12/Vulkan on Windows) with a custom
widget/layout layer — the Zed/GPUI approach. Confirmed by the user over egui, Iced and
Tauri, because the region-capture overlay, the annotation canvas and mixed-DPI multi-monitor
handling all require direct GPU control.

- Capture overlay is a separate borderless window, not a view inside the main window.
- Editor renders straight to a GPU canvas.
- Modules (video, OCR, upload, editor, …) ship and version **independently** and update
  separately, DBeaver-style. Core and each platform adapter also carry their own version.
  This is an architectural commitment, not a preference.

Build order: macOS and Linux first, as equals; Windows adapter follows against the same Core.


## Users

**Primary:** developers and operators who use ShareX on Windows and have no
equivalent on macOS or Linux. They capture, annotate and share screenshots many
times a day as part of communicating about software — bug reports, code review
comments, documentation, client messages. Situation: mid-task, keyboard-driven,
wants the round trip from keystroke to shareable link to take seconds and never
leave the keyboard.

**User zero:** the author (Metanetsoft), working across a MacBook and a Linux
machine, needing the identical tool on both.

## Product Purpose

FoxShot captures a screen region, window, display or recording; lets the user
annotate it immediately; and puts it either on local disk or behind a URL — in
one uninterrupted flow bound to a hotkey. Success is the user stopping to think
about the tool at all.

Two capabilities are v1; everything else is designed now and built later:

1. screenshot capture → in-place editor
2. upload (Cloudflare R2 first; Amazon S3 and free/anonymous hosts alongside)

## Positioning

The mechanism a neighbouring tool cannot cheaply copy: **per-module independent
updates on a native Rust core.** Video, OCR, uploader and editor each version and
update separately, so a fix ships without a whole-app release and a user can run
the tool without modules they do not want. ShareX, CleanShot X and Flameshot all
ship one monolithic binary.

Secondary position: genuinely the same tool on macOS and Linux, native on both —
not an Electron shell, not a Windows app in a compatibility layer.

## Operating Context

- Invoked by global hotkey while another application holds focus. The app itself
  is usually invisible.
- **macOS:** requires Screen Recording permission for capture and Accessibility
  for global hotkeys. Both must be requested explicitly and granted before
  capture works; grants may need re-confirming after an app update.
- **Linux:** X11 and Wayland diverge. Wayland requires xdg-desktop-portal for
  both screenshots and global shortcuts; clipboard requires wl-clipboard/xclip.
- Multi-monitor with mixed DPI is the normal case, not an edge case.
- Captures save by default into the user's Documents folder.
- Multiple accounts/profiles per upload destination where the destination allows it.
- **App shell:** the menu-bar item is the daily surface (capture actions, hotkeys,
  recent captures); a full window holds library, editor, module manager and
  settings. Confirmed by the user over window-only and menu-bar-only alternatives.

## Capabilities and Constraints

Confirmed capability set — all native, never shelling out to a system screenshot
binary as the primary path:

- capture: region, window, full screen, active display, repeat-last-region
- screen recording (video) and GIF
- OCR
- post-capture editor with ShareX-class annotation: shapes, arrows, text, step
  numbers, blur/pixelate, highlight, magnify, spotlight, freehand, crop
- upload destinations: Cloudflare R2, Amazon S3, free/anonymous hosts; multiple
  accounts per destination
- after-capture routing: the capture goes to upload or to disk depending on which
  action was pressed, with a configurable default
- library/history of past captures
- first-run permission wizard: stepped, next/skip, requests each OS permission in
  order and explains why before asking
- per-module update manager

Constraints:

- dark mode only; no light theme
- Roboto is the typeface
- target the current macOS release; older versions supported only where free
- v1 scope is capture + editor + upload. Video, OCR, GIF and recording are
  designed now, built later.

Undecided, and not to be invented: any hosted service or account system, pricing,
the exact free-host list, the Windows timeline beyond "after macOS and Linux".

## Brand Commitments

- Name **FoxShot**. Publisher **Metanetsoft**. Public repository.
- **Licence — open decision.** The user's intent is source-available with
  commercial rights reserved to Metanetsoft: anyone may read, use and modify;
  nobody but Metanetsoft may sell. That intent is **not** GPL-3.0 and not OSI
  open source, because GPL-3.0 explicitly permits anyone to sell. Achieving it
  requires a source-available licence (PolyForm Noncommercial, BSL, or a custom
  EULA) *and* requires FoxShot not to be derived from ShareX's GPL-3.0 code —
  consistent with the stated plan of writing it from scratch in Rust. Flagged for
  confirmation before publishing; not resolved here.
- Dark-only and Roboto are binding visual constraints stated by the user.

## Evidence on Hand

- **No product assets exist yet:** no logo, no icon, no screenshots, no site, no
  users, no testimonials, no benchmarks. Future work must fabricate none of these.
- Reference material examined for competitive and technical grounding only, with
  no code reuse: ShareX (GPL-3.0, ~216k lines C#, Windows-only) and ShareX/XerahS
  (Avalonia cross-platform port shipping macOS/Linux builds; documented macOS gaps
  in KNOWN_ISSUES.md and XIP0078).

## Product Principles

1. **The hotkey is the product.** Every decision is measured against the
   keystroke-to-link path; anything adding a step must justify itself.
2. **Invisible by default.** The app's normal state is not being on screen.
   Surfaces appear on demand and leave.
3. **Modules are independent, and the UI must show it.** A module's version,
   update and absence are first-class states, not error dialogs.
4. **Permission is explained, then requested.** The OS refuses silently; the user
   must never discover a missing grant through a broken screenshot.
5. **One product on every OS, honest about each.** Shared design language;
   platform differences (Wayland portals, macOS TCC) surfaced plainly, not hidden.

## Accessibility & Inclusion

- Full keyboard operation is a functional requirement, not an accommodation — the
  tool is keyboard-driven by nature. Every action reachable without a mouse;
  focus always visible.
- Dark-only means contrast is earned inside one theme: body text and control
  labels clear WCAG AA against their actual surfaces.
- Annotation colour is never the sole carrier of meaning in the UI chrome.
- Reduced-motion preference respected for overlay and panel transitions.
