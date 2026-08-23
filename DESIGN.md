# Design

Documented from the built prototype at `design/foxshot.html`, not written ahead of it.

## World

**Warm consumer-app surface, dark-inverted.** Soft-cornered cards raised on a warm
coffee-black ground, where exactly one saturated colour appears and it always means
"you can press this". The world was chosen by the user from the dealt challenger hand
(seed `0a8fa6a4`) over the roll's assignment, then inverted to dark because dark-only is
a pinned product constraint. The inversion preserved the world's relationships — ground
darker than surface, surface darker than card; warm tint throughout; one action hue —
and deliberately refused cool near-black (`#0d0d0f`) with a neon accent, which is the
category's default and what this build exists to avoid.

## Colour

Every value is warm-biased. There is no pure grey anywhere.

| Token | Value | Role |
|---|---|---|
| `--ground` | `#14100C` | app ground, window body |
| `--surface` | `#1C1713` | recessed: toolbars, side panels |
| `--well` | `#181410` | inputs, deepest recess |
| `--card` | `#241D17` | raised card — lighter than its ground |
| `--card-hi` | `#2C241C` | hover, chips |
| `--border` / `--border-lo` / `--border-hi` | `#382D24` / `#2A2219` / `#4A3C30` | warm hairlines |
| `--ink` / `--ink-2` / `--ink-3` / `--ink-4` | `#F7EFE6` / `#A99783` / `#7B6B5B` / `#5C4F43` | text ramp |
| `--action` | `#FF6A3D` | **only** on actionable elements and the record-armed state |
| `--ok` / `--warn` / `--bad` | `#63B98C` / `#D9A036` / `#E0574C` | semantic state, deliberately desaturated |

**The action rule is the system's spine.** `--action` never decorates. It appears on
primary buttons, the active dock item, the active tool, focused inputs, selection
handles, the capture frame, findings markers and the record indicator — nothing else.
Semantic colours are held below the action hue in saturation so it stays the only
saturated thing on screen, and each is always paired with a text label so colour is
never the sole carrier of meaning.

## Type

Roboto only (pinned by the user), plus Roboto Mono for measured quantities.

- Display 40/700 · Heading 22/700 (`-.02em`) · Subhead 17/500 · Body 15/400 ·
  Label 13/500 · Caption 12/400
- Overline: 11/500, `.09em`, uppercase — used for field-group legends, never as an
  eyebrow above a heading.
- **Roboto Mono is reserved for quantities**: pixel dimensions, file sizes, versions,
  catalogue ids, timestamps, coordinates, hotkeys. It is never a costume for
  "technical"; if a string is not measured, it is not mono.
- Prose blocks stay under ~64ch. Headings take `text-wrap: balance`.

## Components

- **Card** — `--card` on its ground, 18px radius, `--border-lo` hairline, shadow with
  both offset and soft blur. Cards are never nested.
- **Icon chip** — 34px rounded square, `--card-hi`, warm border; takes `--action-tint`
  when its subject is active. The Core module's chip is the only permanently active one.
- **Button** — pill. Primary is `--action` on `#1A0C05`; secondary is a hairline outline;
  ghost is text only. Disabled loses the action colour entirely.
- **Row** — chip · title/sublabel · right cluster · chevron. The library list, module
  list, destination list and settings all reuse this one anatomy.
- **Badge** — pill with a dot, one per semantic role plus a muted variant.
- **Dock** — the world's persistent nav bar, translated from a phone's thumb-reachable
  bottom bar to a desktop command bar at the foot of the window: five destinations with
  `⌘1`–`⌘5` shown in mono on the right. This is the build's one deliberate risk.
- **Findings list** — the editor's right rail. Placing a mark writes a row; the row
  carries the mark's number and coordinates. The annotation list is a readable document,
  not a layer panel. This is the signature interaction.

Icons are hand-authored SVG in one 24-box, 1.75 stroke, rounded caps, as a `<symbol>`
sprite. No emoji, no icon font. Any `ic()` call without an explicit class is sized by a
zero-specificity `:where(svg[class=""])` guard so an unsized icon can never inflate to
the SVG default box again.

## Platform shells

One Core, three shells. `data-os` on the stage swaps only chrome: macOS traffic lights
left; GNOME round header buttons right; Windows square caption buttons right and flush.
Window radius and title-bar height shift per platform; modifier keys become `⌘` or
`Ctrl`; permission language becomes TCC, xdg-desktop-portal, or Graphics Capture. Every
pixel of content between the chrome is identical, which is the architecture's claim made
visible.

## Motion

One authored moment: the menu-bar panel, which scales and lifts from its tray origin on
`cubic-bezier(.16,1,.3,1)`. Everything else is a short state change — 130–190ms, no
bounce, no entrance animation on content. Progress bars ease their width. The record
indicator is the only looping animation. `prefers-reduced-motion` collapses all of it.

## Copy

Turkish, plain, from the operator's side of the screen. Controls name their action
("Yükle ve linki kopyala", then a toast that says "Link kopyalandı"). Errors name the
problem, its cause and the recovery in that order — "Gizli anahtar reddedildi.
Cloudflare 403 döndü — anahtar silinmiş ya da bu bucket'a yazma izni yok. R2 panelinden
yeni bir anahtar üretip buraya yapıştır." Reassurance is stated explicitly where a user
would fear loss: a failed module update says the old version keeps running; deleting
captures says uploaded links are unaffected.

## Refused

Kickers and eyebrows. Nested cards. Same-size icon-heading-text card grids as page
structure. Gradient text. Glass and blur as decoration. Coloured left borders. Zero-blur
block shadows. Progress rings and sparklines standing in for content. Section numbers.
Mono as a technical costume. Cool near-black with a neon accent.

## Still placeholder

- The fox mark is an authored placeholder, not a logo — `PRODUCT.md` records that no
  brand assets exist yet.
- All captures shown inside the prototype are synthetic SVG drawings, labelled as such
  in the source. Replace with real screenshots before any public use.
- No real endpoints, prices, customers or benchmarks appear anywhere, by design.
