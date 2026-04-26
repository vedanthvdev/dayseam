import type { SVGProps } from "react";
import type { SourceKind } from "@dayseam/ipc-types";

/**
 * DAY-159. Small inline-SVG brand mark for each connector kind,
 * keyed on the canonical {@link SourceKind} enum so there is a
 * single place to extend when a new connector lands.
 *
 * Design rationale:
 * - `fill="currentColor"` so the mark inherits the chip's text
 *   colour and dark-mode classes automatically, the same way every
 *   other glyph in the app already themes.
 * - Path data is inlined (no HTTP, no asset pipeline, no SVGR, no
 *   runtime dep on `simple-icons`) because five paths is easier to
 *   audit than a toolchain, and the marks themselves change on the
 *   order of once-per-decade.
 * - All five paths are sourced from Simple Icons (CC0), which
 *   tracks each service's official brand mark. See `CREDITS.md`
 *   additions in this PR for attribution; the marks themselves
 *   remain the trademarks of their respective owners and are used
 *   here in the classic "connected to X" nominative-fair-use sense.
 *
 * Accessibility:
 * - Default is **decorative** (`aria-hidden`, no `<title>`): most
 *   callers already render a visible text label next to the mark,
 *   so exposing the brand name to screen readers is redundant alt
 *   text.
 * - Callers that render the mark *without* a visible label should
 *   set `labelled={true}`; that turns on `role="img"` and a
 *   `<title>` so assistive tech announces the service.
 */

const GITHUB_PATH =
  "M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12";

const GITLAB_PATH =
  "m23.6004 9.5927-.0337-.0862L20.3.9814a.851.851 0 0 0-.3362-.405.8748.8748 0 0 0-.9997.0539.8748.8748 0 0 0-.29.4399l-2.2055 6.748H7.5375l-2.2057-6.748a.8573.8573 0 0 0-.29-.4412.8748.8748 0 0 0-.9997-.0537.8585.8585 0 0 0-.3362.4049L.4332 9.5015l-.0325.0862a6.0657 6.0657 0 0 0 2.0119 7.0105l.0113.0087.03.0213 4.976 3.7264 2.462 1.8633 1.4995 1.1321a1.0085 1.0085 0 0 0 1.2197 0l1.4995-1.1321 2.4619-1.8633 5.006-3.7489.0125-.01a6.0682 6.0682 0 0 0 2.0094-7.003z";

const JIRA_PATH =
  "M12.004 0c-2.35 2.395-2.365 6.185.133 8.585l3.412 3.413-3.197 3.198a6.501 6.501 0 0 1 1.412 7.04l9.566-9.566a.95.95 0 0 0 0-1.344L12.004 0zm-1.748 1.74L.67 11.327a.95.95 0 0 0 0 1.344C4.45 16.44 8.22 20.244 12 24c2.295-2.298 2.395-6.096-.08-8.533l-3.47-3.469 3.2-3.2c-1.918-1.955-2.363-4.725-1.394-7.057z";

const CONFLUENCE_PATH =
  "M.87 18.257c-.248.382-.53.875-.763 1.245a.764.764 0 0 0 .255 1.04l4.965 3.054a.764.764 0 0 0 1.058-.26c.199-.332.454-.763.733-1.221 1.967-3.247 3.945-2.853 7.508-1.146l4.957 2.337a.764.764 0 0 0 1.028-.382l2.364-5.346a.764.764 0 0 0-.382-1 599.851 599.851 0 0 1-4.965-2.361C10.911 10.97 5.224 11.185.87 18.257zM23.131 5.743c.249-.405.531-.875.764-1.25a.764.764 0 0 0-.256-1.034L18.675.404a.764.764 0 0 0-1.058.26c-.195.335-.451.763-.734 1.225-1.966 3.246-3.945 2.85-7.508 1.146L4.437.694a.764.764 0 0 0-1.027.382L1.046 6.422a.764.764 0 0 0 .382 1c1.039.49 3.105 1.467 4.965 2.361 6.698 3.246 12.392 3.029 16.738-4.04z";

const GIT_PATH =
  "M23.546 10.93L13.067.452c-.604-.603-1.582-.603-2.188 0L8.708 2.627l2.76 2.76c.645-.215 1.379-.07 1.889.441.516.515.658 1.258.438 1.9l2.658 2.66c.645-.223 1.387-.078 1.9.435.721.72.721 1.884 0 2.604-.719.719-1.881.719-2.6 0-.539-.541-.674-1.337-.404-1.996L12.86 8.955v6.525c.176.086.342.203.488.348.713.721.713 1.883 0 2.6-.719.721-1.889.721-2.609 0-.719-.719-.719-1.879 0-2.598.182-.18.387-.316.605-.406V8.835c-.217-.091-.424-.222-.6-.401-.545-.545-.676-1.342-.396-2.009L7.636 3.7.45 10.881c-.6.605-.6 1.584 0 2.189l10.48 10.477c.604.604 1.582.604 2.186 0l10.43-10.43c.605-.603.605-1.582 0-2.187";

// DAY-202. Microsoft Outlook mark (Simple Icons, CC0). Used by the
// Outlook connector landing in v0.9. No source rows can render this
// mark until DAY-203 adds the Add-Source dialog; the entry exists
// here so the `Record<SourceKind, MarkDef>` stays exhaustive and the
// frontend typechecks.
const OUTLOOK_PATH =
  "M7.88 12.04q0 .45-.11.87-.1.41-.33.74-.22.33-.58.52-.37.2-.87.2t-.85-.2q-.35-.21-.57-.55-.22-.33-.33-.75-.1-.42-.1-.86t.1-.87q.1-.43.34-.76.22-.34.59-.54.36-.2.87-.2t.86.2q.35.21.57.55.22.34.31.77.1.43.1.88zM24 12v9.38q0 .46-.33.8-.33.32-.8.32H7.13q-.46 0-.8-.33-.32-.33-.32-.8V18H1q-.41 0-.7-.3-.3-.29-.3-.7V7q0-.41.3-.7Q.58 6 1 6h6.5V2.55q0-.44.3-.75.3-.3.75-.3h12.9q.44 0 .75.3.3.3.3.75V10.85l1.24.72h.01q.1.07.18.18.07.12.07.25zm-6-8.25v3h3v-3zm0 4.5v3h3v-3zm0 4.5v1.83l3.05-1.83zm-5.25-9v3h3.75v-3zm0 4.5v3h3.75v-3zm0 4.5v2.03l2.41 1.5 1.34-.8v-2.73zM9 3.75V6h2l.13.01.12.04v-2.3zM5.98 15.98q.9 0 1.6-.3.7-.32 1.19-.86.48-.55.73-1.28.25-.74.25-1.61 0-.83-.25-1.55-.24-.71-.71-1.24t-1.15-.83q-.68-.3-1.55-.3-.92 0-1.64.3-.71.3-1.2.85-.5.54-.75 1.3-.25.74-.25 1.63 0 .85.26 1.56.26.72.74 1.23.48.52 1.17.81.69.3 1.56.3zM7.5 21h12.39L12 16.08V17q0 .41-.3.7-.29.3-.7.3H7.5zm15-.13v-7.24l-5.9 3.54Z";

interface MarkDef {
  /** The path `d` attribute of the brand mark. */
  readonly path: string;
  /** Human-facing brand name for screen readers when `labelled`. */
  readonly brandName: string;
  /** Brand accent colour for light/dark mode. Consumers opt in with
   *  `colored` — by default the mark still inherits `currentColor`
   *  so existing layouts keep their neutral monochrome look. */
  readonly accent: { readonly light: string; readonly dark: string };
}

// DAY-170 brand-accent palette.
//
// Choices are pinned per-mode rather than "one hex that works
// everywhere" because the two pain cases — GitHub's near-black mark
// on a dark surface, and the Atlassian navy on a light surface — only
// resolve when we can flip per theme. Values are drawn from each
// service's own dark-mode / light-mode renderings so a user who
// recognises the brand anywhere else in their day recognises it here.
//
// Colour policy, intentionally aligned with the marketing site's own
// connector grid (canonical home: dayseam/dayseam.github.io under
// `src/data/connectors.ts`). The two files duplicate the per-connector
// hexes on purpose — the site ships standalone from GitHub Pages so
// pulling them into a shared package would require a published
// `@dayseam/ui` that the Pages repo can install. When Simple Icons
// upstream a brand tweak, change both repos in the same change set;
// there is no CI gate spanning the two today.
// - GitHub:     #24292F / #F0F6FC — GitHub's own fg for each surface.
// - GitLab:     #FC6D26 / #FC6D26 — canonical tangerine, legible on
//               both light and dark backgrounds without substitution.
// - Jira:       #0052CC / #4C9AFF — Atlassian's Jira-primary blue in
//               light mode; Atlassian sky in dark mode (the deeper
//               blue goes muddy against #0a0a0a).
// - Confluence: #172B4D / #2684FF — Atlassian's canonical Confluence
//               navy on light surfaces; the brighter Atlassian sky
//               in dark mode so the mark doesn't disappear and the
//               Jira/Confluence pair stays visually distinct (Jira
//               slightly darker, Confluence slightly lighter).
// - LocalGit:   #F05032 / #F05032 — Git's canonical red-orange.
const MARKS: Record<SourceKind, MarkDef> = {
  GitHub: {
    path: GITHUB_PATH,
    brandName: "GitHub",
    accent: { light: "#24292F", dark: "#F0F6FC" },
  },
  GitLab: {
    path: GITLAB_PATH,
    brandName: "GitLab",
    accent: { light: "#FC6D26", dark: "#FC6D26" },
  },
  Jira: {
    path: JIRA_PATH,
    brandName: "Jira",
    accent: { light: "#0052CC", dark: "#4C9AFF" },
  },
  Confluence: {
    path: CONFLUENCE_PATH,
    brandName: "Confluence",
    accent: { light: "#172B4D", dark: "#2684FF" },
  },
  // Use the canonical Git mark for LocalGit. Git is the wire
  // protocol the LocalGit connector walks, and there is no separate
  // "Dayseam local repo" identity to invent a mark for. When the
  // app-icon work lands a Dayseam mark, this row stays unchanged —
  // the Dayseam mark identifies the app; this mark identifies the
  // source kind.
  LocalGit: {
    path: GIT_PATH,
    brandName: "Local Git repository",
    accent: { light: "#F05032", dark: "#F05032" },
  },
  // DAY-202. Microsoft Outlook brand accent is #0078D4 — the
  // canonical Microsoft product blue, which reads cleanly on both
  // light and dark surfaces without a per-mode swap. No call site
  // renders this entry yet; the Add-Source dialog that introduces
  // Outlook rows lands in DAY-203.
  Outlook: {
    path: OUTLOOK_PATH,
    brandName: "Microsoft Outlook",
    accent: { light: "#0078D4", dark: "#0078D4" },
  },
};

/**
 * Return the accent hex pair for `kind`. Exposed so non-SVG call
 * sites (e.g. a chip that wants a faint coloured border, or the
 * identity row's coloured dot) can share the single source of truth
 * for what "the GitHub colour" is without poking at the internals
 * of this module.
 */
export function connectorAccent(kind: SourceKind): {
  light: string;
  dark: string;
} {
  return MARKS[kind].accent;
}

export interface ConnectorLogoProps
  extends Omit<SVGProps<SVGSVGElement>, "children" | "viewBox" | "fill"> {
  /** Canonical connector kind — keys into {@link MARKS}. */
  kind: SourceKind;
  /** Pixel size for both width and height. Defaults to 14, matching
   *  the text-xs metrics of the sources sidebar chip. */
  size?: number;
  /** When `true`, the logo is announced to assistive tech via
   *  `role="img"` plus a `<title>` holding the brand name. Leave
   *  `false` (the default) when a visible text label already names
   *  the service — otherwise the mark is redundant alt text. */
  labelled?: boolean;
  /**
   * When `true`, render the mark in its brand accent instead of the
   * chip's `currentColor`. DAY-170 wired this on so the sources
   * sidebar, the Add-source dropdown, and the identity manager can
   * all surface the one visual signal users already know — GitHub
   * is near-black in light mode and white in dark mode, GitLab is
   * tangerine, and so on — without any other glyph on the page
   * picking up bespoke colour. The flag exists as an opt-in rather
   * than a global switch because the same component still has to
   * serve monochrome callers (e.g. the log-drawer run rows) where a
   * colour flash would look noisy.
   *
   * The colour is switched at runtime via CSS's native
   * `light-dark()` function, relying on `color-scheme: light|dark`
   * being set on `<html>` by `applyResolvedTheme`. That means the
   * mark flips the instant the app's theme changes without the
   * component needing a React subscription to `useTheme()`, and it
   * inherits the user's system theme correctly when the preference
   * is `system`. Tauri 2 ships a recent-enough WebView on every
   * supported platform that `light-dark()` is safe to rely on; if
   * the function is ever unsupported, the browser falls through to
   * the inherited `currentColor` and the mark still renders — just
   * in a neutral tone rather than in brand accent.
   */
  colored?: boolean;
}

/** Inline-SVG brand mark for the given {@link SourceKind}. See the
 *  file-level doc for rationale and accessibility guidance. */
export function ConnectorLogo({
  kind,
  size = 14,
  labelled = false,
  colored = false,
  className,
  style,
  ...rest
}: ConnectorLogoProps): JSX.Element {
  const mark = MARKS[kind];
  const coloredStyle = colored
    ? // Using `color` (not `fill`) so the `fill="currentColor"`
      // below picks up the tint automatically; this keeps the rest
      // of the component (accessibility, sizing) identical whether
      // or not the caller wants the brand accent.
      {
        color: `light-dark(${mark.accent.light}, ${mark.accent.dark})`,
      }
    : undefined;
  return (
    <svg
      role={labelled ? "img" : undefined}
      aria-hidden={labelled ? undefined : true}
      aria-label={labelled ? mark.brandName : undefined}
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="currentColor"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      style={{ ...coloredStyle, ...style }}
      data-testid={`connector-logo-${kind}`}
      data-colored={colored ? "true" : undefined}
      // DAY-170: expose the resolved accent pair as data attributes
      // when `colored` is on. This exists specifically for two
      // consumers that cannot observe the inline `color:
      // light-dark(...)` cleanly:
      //   1. JSDOM-backed vitest runs, where `style.color` drops
      //      values it doesn't parse (including `light-dark()`), so
      //      tests would otherwise have to reach into CSSOM
      //      internals or diff snapshots just to assert the right
      //      brand hex landed.
      //   2. Playwright / a11y smoke checks that want to target
      //      "the GitHub-coloured chip" without parsing computed
      //      styles back into hexes.
      // The attributes are intentionally absent when `colored` is
      // false so the monochrome render stays clean.
      data-accent-light={colored ? mark.accent.light : undefined}
      data-accent-dark={colored ? mark.accent.dark : undefined}
      {...rest}
    >
      {labelled ? <title>{mark.brandName}</title> : null}
      <path d={mark.path} />
    </svg>
  );
}
