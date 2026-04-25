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

interface MarkDef {
  /** The path `d` attribute of the brand mark. */
  readonly path: string;
  /** Human-facing brand name for screen readers when `labelled`. */
  readonly brandName: string;
}

const MARKS: Record<SourceKind, MarkDef> = {
  GitHub: { path: GITHUB_PATH, brandName: "GitHub" },
  GitLab: { path: GITLAB_PATH, brandName: "GitLab" },
  Jira: { path: JIRA_PATH, brandName: "Jira" },
  Confluence: { path: CONFLUENCE_PATH, brandName: "Confluence" },
  // Use the canonical Git mark for LocalGit. Git is the wire
  // protocol the LocalGit connector walks, and there is no separate
  // "Dayseam local repo" identity to invent a mark for. When the
  // app-icon work lands a Dayseam mark, this row stays unchanged —
  // the Dayseam mark identifies the app; this mark identifies the
  // source kind.
  LocalGit: { path: GIT_PATH, brandName: "Local Git repository" },
};

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
}

/** Inline-SVG brand mark for the given {@link SourceKind}. See the
 *  file-level doc for rationale and accessibility guidance. */
export function ConnectorLogo({
  kind,
  size = 14,
  labelled = false,
  className,
  ...rest
}: ConnectorLogoProps): JSX.Element {
  const mark = MARKS[kind];
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
      data-testid={`connector-logo-${kind}`}
      {...rest}
    >
      {labelled ? <title>{mark.brandName}</title> : null}
      <path d={mark.path} />
    </svg>
  );
}
