// Collapsible error card rendered below a source chip when its
// `health.last_error` code is a known `gitlab.*`, `atlassian.*`,
// `jira.*`, or `confluence.*` code. Lifts the error payload from a
// noisy hover-tooltip (chip title) into a surface that can explain
// what happened and invite a recovery action — specifically, the
// "Reconnect" deep link that re-opens the appropriate add-source
// dialog for the auth codes.
//
// The card is intentionally tiny: one headline, one body, and at
// most one action button. The copy comes from `gitlabErrorCopy.ts`
// and `atlassianErrorCopy.ts`, both parity-tested against
// `dayseam_core::error_codes::ALL`, so every supported code that
// ships in Rust has a string here.
//
// Unknown codes (anything outside the copy maps) render a generic
// fallback rather than nothing — the red chip already tells the
// user something failed; hiding the card for an unmapped code would
// be more confusing than showing a plain "Something went wrong"
// with the raw error code as diagnostic copy.

import type {
  AtlassianErrorCode,
  DayseamError,
  GithubErrorCode,
  GitlabErrorCode,
  Source,
} from "@dayseam/ipc-types";
import {
  ATLASSIAN_ERROR_CODES,
  GITHUB_ERROR_CODES,
  GITLAB_ERROR_CODES,
} from "@dayseam/ipc-types";
import { atlassianErrorCopy } from "./atlassianErrorCopy";
import { githubErrorCopy } from "./githubErrorCopy";
import { gitlabErrorCopy } from "./gitlabErrorCopy";

interface SourceErrorCardProps {
  source: Source;
  error: DayseamError;
  /** Fired when the user clicks "Reconnect" for an auth code. */
  onReconnect: (source: Source) => void;
}

const GITLAB_ERROR_CODE_SET: ReadonlySet<string> = new Set(GITLAB_ERROR_CODES);
const ATLASSIAN_ERROR_CODE_SET: ReadonlySet<string> = new Set(
  ATLASSIAN_ERROR_CODES,
);
const GITHUB_ERROR_CODE_SET: ReadonlySet<string> = new Set(GITHUB_ERROR_CODES);

function isGitlabErrorCode(code: string): code is GitlabErrorCode {
  return GITLAB_ERROR_CODE_SET.has(code);
}

function isAtlassianErrorCode(code: string): code is AtlassianErrorCode {
  return ATLASSIAN_ERROR_CODE_SET.has(code);
}

function isGithubErrorCode(code: string): code is GithubErrorCode {
  return GITHUB_ERROR_CODE_SET.has(code);
}

type CardCopy = {
  title: string;
  body: string;
  action: "reconnect" | "retry" | "none";
};

function resolveCopy(code: string): CardCopy | null {
  if (isGitlabErrorCode(code)) return gitlabErrorCopy[code];
  if (isAtlassianErrorCode(code)) return atlassianErrorCopy[code];
  if (isGithubErrorCode(code)) return githubErrorCopy[code];
  return null;
}

export function SourceErrorCard({
  source,
  error,
  onReconnect,
}: SourceErrorCardProps) {
  const code = error.data.code;
  const copy = resolveCopy(code);

  const title = copy?.title ?? "Something went wrong";
  const body =
    copy?.body ??
    `Dayseam couldn't sync this source. The error code was ${code}.`;
  const action = copy?.action ?? "none";

  return (
    <div
      role="group"
      aria-label={`Error details for ${source.label}`}
      data-testid={`source-error-card-${source.id}`}
      data-error-code={code}
      className="mt-1 w-full rounded border border-red-300 bg-red-50 px-2 py-1.5 text-xs text-red-900 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200"
    >
      <div className="flex items-start justify-between gap-2">
        <div>
          <p className="font-medium">{title}</p>
          <p className="mt-0.5 text-red-800 dark:text-red-300">{body}</p>
          <p className="mt-0.5 font-mono text-[10px] text-red-700/80 dark:text-red-300/70">
            {code}
          </p>
        </div>
        {action === "reconnect" ? (
          <button
            type="button"
            onClick={() => onReconnect(source)}
            data-testid={`source-error-card-reconnect-${source.id}`}
            className="shrink-0 rounded border border-red-400 bg-white px-2 py-0.5 text-[11px] font-medium text-red-700 hover:bg-red-100 dark:border-red-700 dark:bg-red-900/40 dark:text-red-100 dark:hover:bg-red-900"
          >
            Reconnect
          </button>
        ) : null}
      </div>
    </div>
  );
}
