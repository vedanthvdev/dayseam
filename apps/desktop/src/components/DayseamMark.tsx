import type { SVGProps } from "react";

/**
 * DAY-170: inline background-less variant of the Dayseam brand
 * mark (`assets/brand/dayseam-mark.svg`, "Convergence"). The
 * canonical file packs the strands into a rounded charcoal square
 * so the OS dock icon reads — but every in-app use of the mark
 * would drop a little charcoal box into the chrome, which the
 * brand explicitly avoids in any embedded context. This component
 * renders the coloured strands and seam directly onto whatever
 * surface it sits on, so the mark floats in the title bar beside
 * the wordmark without a frame.
 *
 * Geometry is byte-for-byte identical to the canonical SVG — same
 * convergence point (680, 512), same fan angles (-36°, -18°, 0°,
 * +18°, +36°), same dash pattern (44/24) on the seam. If the
 * `dayseam-mark.svg` source ever shifts, regenerate this file
 * from it; do not redraw.
 *
 * The seam (cream in the rounded-square variant, where it reads
 * against #17171A) flips to the current text colour here because
 * cream against a white title bar is invisible. The strand palette
 * stays fixed — the mark is the one place in the app where brand
 * accent is the whole point, so we pin the five hues rather than
 * letting them theme.
 *
 * `aria-hidden` by default since the TitleBar already renders
 * "Dayseam" as visible text next to the mark, so exposing the
 * brand name to screen readers would be redundant. Pass
 * `labelled` to flip to `role="img"` with a `<title>` for surfaces
 * that render the mark without a visible word.
 */
export interface DayseamMarkProps
  extends Omit<SVGProps<SVGSVGElement>, "children" | "viewBox" | "fill"> {
  /** Width and height in CSS pixels. Defaults to 20 — one step
   *  larger than the chip marks so it reads as the app's identity
   *  rather than as just another connector row. */
  size?: number;
  /** When `true`, announce "Dayseam" to assistive tech via a
   *  `<title>`. Default `false` because the title bar already
   *  carries the wordmark. */
  labelled?: boolean;
}

export function DayseamMark({
  size = 20,
  labelled = false,
  className,
  ...rest
}: DayseamMarkProps): JSX.Element {
  return (
    <svg
      role={labelled ? "img" : undefined}
      aria-hidden={labelled ? undefined : true}
      aria-label={labelled ? "Dayseam" : undefined}
      viewBox="0 0 1024 1024"
      width={size}
      height={size}
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      data-testid="dayseam-mark"
      {...rest}
    >
      {labelled ? <title>Dayseam</title> : null}
      {/*
        Stroke width is bumped to 72 here from the canonical 28 the
        full-size mark uses, because at chrome-scale (20 px default)
        a 28 stroke against a 1024 viewBox rasterises to ~0.55 CSS
        px and effectively disappears on low-DPI displays. 72 / 1024
        × 20 ≈ 1.4 px of rendered ink — still delicate, still reads
        as a "thin" mark, but survives low-DPI without fading. The
        icon-file rendering that ships to the OS dock continues to
        use the canonical 28 because its own raster size makes the
        thinner stroke correct.
      */}
      <g strokeWidth={72} strokeLinecap="round" fill="none">
        <line x1={292} y1={230} x2={680} y2={512} stroke="#E89A2C" />
        <line x1={224} y1={364} x2={680} y2={512} stroke="#2B8AA0" />
        <line x1={200} y1={512} x2={680} y2={512} stroke="#D94F6E" />
        <line x1={224} y1={660} x2={680} y2={512} stroke="#5BA567" />
        <line x1={292} y1={794} x2={680} y2={512} stroke="#4D6DD0" />
        {/*
          The seam takes `currentColor` rather than the cream
          `#F6F1E6` the rounded-square variant uses. Cream on cream
          (title bar is white in light mode) would disappear; cream
          on charcoal only works in the icon file. Inheriting the
          title bar's ink keeps the seam legible on both themes and
          signals that the seam is the "app" side of the mark while
          the strands are the "sources" side.
        */}
        <line
          x1={680}
          y1={512}
          x2={980}
          y2={512}
          stroke="currentColor"
          strokeDasharray="44 24"
        />
      </g>
    </svg>
  );
}
