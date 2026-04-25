# Dayseam brand

This is the one-pager brand guide for Dayseam. The locked brand mark
and palette were agreed in
[issue #161](https://github.com/dayseam/dayseam/issues/161); this doc
exists so future contributors do not redraw, recolour, or repurpose the
mark off the cuff.

If you are about to do anything that puts the Dayseam name or logo in
front of a user, read this page first.

## The mark

The Dayseam mark is called **"Convergence"**. Five thin strands enter
from the upper-left at evenly varied angles and meet at a single
anchor point on the right, exiting as a horizontal stitched seam. The
narrative is deliberate: many sources gather at one point and are
stitched into a single record, which is exactly what the app does.

Source files (canonical, do not edit downstream rasters):

- [`assets/brand/dayseam-mark.svg`](../../assets/brand/dayseam-mark.svg)
  &mdash; the primary, full-colour, on-charcoal version. 1024x1024
  viewbox. Use this as the source of truth for every raster.
- [`assets/brand/dayseam-mark-mono.svg`](../../assets/brand/dayseam-mark-mono.svg)
  &mdash; same geometry, single ink, transparent background, stroke
  set to `currentColor`. Use this in single-ink contexts: favicons,
  menubar/tray template images, README badges, printed contexts, email
  signatures.

Both files share byte-identical geometry. If one drifts, treat the
full-colour version as the source of truth and re-derive the
monochrome variant from it.

## Palette

The palette is the bolder editorial set locked in DAY-161. Confident
hues that hold up against a dark surface, but never saturated to the
point of looking like a tech-rainbow.

| Token            | Hex      | Used for                                |
| ---------------- | -------- | --------------------------------------- |
| Background       | `#17171A`| Icon background (rounded square), dark mode chrome |
| Strand &mdash; gold   | `#E89A2C`| Topmost strand (-36 deg)                |
| Strand &mdash; teal   | `#2B8AA0`| Strand at -18 deg                       |
| Strand &mdash; coral  | `#D94F6E`| Strand at 0 deg (the horizontal one)    |
| Strand &mdash; sage   | `#5BA567`| Strand at +18 deg                       |
| Strand &mdash; indigo | `#4D6DD0`| Bottom strand (+36 deg)                 |
| Seam cream       | `#F6F1E6`| The horizontal stitched seam, also general light-on-dark text |

These hex codes are the contract. Do not eyeball them, copy them.

## Geometry summary

Useful when you need to recreate the mark in a foreign tool (Figma,
Sketch, a slide deck):

- Canvas: `1024 x 1024`
- Background corner radius: `229` (22.37 percent of side &mdash; matches the
  macOS Big Sur+ continuous-corner feel; baked into the source because
  macOS does *not* auto-round square app icon PNGs).
- Convergence point: `(680, 512)` &mdash; about two thirds across,
  vertical centre.
- Five strands at angles `-36, -18, 0, +18, +36` degrees from
  horizontal, each `480` px long, stroke weight `28`, round line cap.
- Seam: horizontal running stitch from `(680, 512)` to `(980, 512)`
  with `stroke-dasharray="44 24"`, round line cap.

## Where the mark lives in this repo

- `assets/brand/` &mdash; canonical SVG sources (this is the source of truth).
- `apps/desktop/src-tauri/icons/` &mdash; rasterised PNGs at the four sizes
  Tauri's bundle config requires (`32x32.png`, `128x128.png`,
  `128x128@2x.png`, `icon.png`). Regenerate from
  `assets/brand/dayseam-mark.svg` whenever the SVG changes:

  ```bash
  TMPDIR=$(mktemp -d)
  qlmanage -t -s 1024 -o "$TMPDIR" assets/brand/dayseam-mark.svg
  MASTER="$TMPDIR/dayseam-mark.svg.png"

  cp "$MASTER" apps/desktop/src-tauri/icons/icon.png         && sips -Z 512 apps/desktop/src-tauri/icons/icon.png
  cp "$MASTER" apps/desktop/src-tauri/icons/128x128@2x.png   && sips -Z 256 apps/desktop/src-tauri/icons/128x128@2x.png
  cp "$MASTER" apps/desktop/src-tauri/icons/128x128.png      && sips -Z 128 apps/desktop/src-tauri/icons/128x128.png
  cp "$MASTER" apps/desktop/src-tauri/icons/32x32.png        && sips -Z 32  apps/desktop/src-tauri/icons/32x32.png

  rm -rf "$TMPDIR"
  ```

  `qlmanage` and `sips` are both built into macOS, so no extra tooling
  is required.

## Do

- **Use the canonical SVG.** Re-rasterise from
  `assets/brand/dayseam-mark.svg` whenever you need a PNG; do not hand
  redraw the mark in another tool, and do not re-export from a stale
  copy.
- **Respect the corner radius.** When the rounded-square shape is
  needed (app icons, web hero), use the SVG's `<rect rx="229">` as-is.
  Do not crop to a circle, do not square it off to a hard rectangle.
- **Keep clear space around the mark.** Reserve at minimum `1/8`
  of the icon side as empty space on every side when laying out next
  to other content (so on a `1024` mark, that is `128` px of clear
  space).
- **Use the monochrome variant in single-ink contexts.** Favicons at
  16/32 px, dark-mode tray icons, README badges, anywhere a multi-hue
  mark would fight the surrounding UI. Never recolour the multi-hue
  version to a single ink &mdash; use the dedicated mono SVG instead.
- **Pair the mark with the literal wordmark "Dayseam"** when external
  audiences may not yet recognise the icon (marketing site hero, App
  Store listing, social cards). Geometric sans-serif at the same
  optical weight as the strands; do not use Comic Sans, do not use a
  decorative or "stitched" font.

## Don't

- **Don't recolour the strands.** The five hues are part of the
  identity. Do not swap in product accent colours, do not replace the
  cream seam with white, do not give one strand a "highlight" colour
  to emphasise a feature. The mark is one mark.
- **Don't rotate the mark.** The strands fan from the upper-left and
  the seam exits to the right. That orientation is the design. No
  vertical, no diagonal, no kaleidoscope.
- **Don't outline, drop-shadow, or 3D-extrude the mark.** It is a flat
  vector. Effects break the silhouette and drag it toward the
  illustration register.
- **Don't put the mark below 32 px** in user-facing UI. The strand
  detail dissolves and the convergence point reads as a smear; below
  that threshold use the wordmark "Dayseam" alone, or no mark at all.
- **Don't apply the mark to surfaces lighter than the background
  charcoal** without verifying contrast. The seam cream and gold strand
  in particular lose definition on a white surface; in those cases use
  the mono variant in a darker ink.
- **Don't add new strands ad hoc to "represent the new connector"** in
  a one-off context. The strand count is intentionally not a literal
  one-strand-per-connector claim. If a future strand truly belongs in
  the mark, change `assets/brand/dayseam-mark.svg` (and re-rasterise
  every downstream asset) so every surface stays in lockstep &mdash; do
  not fork the mark on a single page or slide.

## Wordmark

The literal text "Dayseam" rendered in a clean geometric sans-serif at
sentence case (`Dayseam`, never `DAYSEAM`, never `dayseam`, never
`day seam`). When the wordmark sits next to the mark, the wordmark's
cap-height should match the inner height of the convergence anchor
point, and there should be at least `64` px of horizontal clear space
between them on a `1024` master.

A formal wordmark lockup file will land in a follow-up; until then,
feel free to set the wordmark in any geometric sans you have to hand
(Inter, Geist, Manrope, SF Pro Display) at `medium` weight, sentence
case.

## Questions or proposed changes

Open an issue with the `documentation` and `enhancement` labels that
references this file directly. Brand changes touch every shipping
surface and need a deliberate decision &mdash; do not bypass.
