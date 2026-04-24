// Normalisation for the Atlassian workspace-URL field in
// `AddAtlassianSourceDialog`.
//
// The IPC layer (`crates/dayseam-core/src/error_codes.rs` →
// `ipc.atlassian.invalid_workspace_url`; server-side helper
// `apps/desktop/src-tauri/src/ipc/atlassian.rs::parse_workspace_url`)
// stores `SourceConfig::{Jira, Confluence}.workspace_url` as a bare
// `https://<host>` origin with no trailing slash and no path. The
// user's input, however, arrives in three shapes we've seen in the
// wild:
//
//   1. `yourcompany`                          — just the subdomain slug
//   2. `https://yourcompany.atlassian.net`     — pasted from the browser bar
//   3. `https://yourcompany.atlassian.net/`    — with a trailing slash
//
// This module is the single place that loose input is collapsed to
// the tight stored shape, so the dialog and its tests can share one
// rulebook. The `workspace_url_normalisation` invariant from the
// plan (§DAY-82 Task 9, "invariants proven by tests") is tested here
// in `__tests__/atlassian-workspace-url.test.ts`.
//
// Unlike the GitLab base-URL helper, Atlassian Cloud is https-only
// — there is no self-hosted Atlassian Cloud (Data Center is a
// separate product with its own URL shapes, not yet supported). We
// reject `http://` loudly rather than silently upgrading to `https`
// so a user who miskeys `http://` sees what they typed.

/** Outcome of normalising whatever the user typed into the workspace box. */
export type WorkspaceUrlNormalisation =
  | { kind: "ok"; url: string }
  | { kind: "empty" }
  | { kind: "invalid"; reason: string };

/**
 * Turn raw user input into a normalised Atlassian workspace URL.
 *
 * Six acceptance cases (matching the table in the plan):
 *
 * 1. `yourcompany`                           → `https://yourcompany.atlassian.net`      (slug prefilled)
 * 2. `https://yourcompany.atlassian.net`     → `https://yourcompany.atlassian.net`      (identity)
 * 3. `https://yourcompany.atlassian.net/`    → `https://yourcompany.atlassian.net`      (trailing slash stripped)
 * 4. `http://yourcompany.atlassian.net`      → `invalid`                                  (cleartext disallowed)
 * 5. `https://yourcompany.atlassian.net/wiki`→ `invalid`                                  (path not allowed)
 * 6. ``                                      → `empty`                                    (submit stays disabled)
 *
 * DAY-127 #5a: the user-facing reason strings were reworded off a
 * specific company name to a neutral `yourcompany` placeholder so
 * the dialog never implies a particular Atlassian tenant is
 * hardcoded in the app.
 */
export function normaliseWorkspaceUrl(raw: string): WorkspaceUrlNormalisation {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return { kind: "empty" };

  // A bare slug (no dot, no scheme) is expanded to the canonical
  // `<slug>.atlassian.net` shape. `new URL` requires a scheme, so
  // the scheme is the thing we prepend — slug validation (no
  // spaces, no scheme characters) happens immediately after.
  let candidate: string;
  if (/^[a-z][a-z0-9+\-.]*:\/\//i.test(trimmed)) {
    candidate = trimmed;
  } else if (/^[a-z0-9][a-z0-9-]*$/i.test(trimmed)) {
    candidate = `https://${trimmed}.atlassian.net`;
  } else if (/^[a-z0-9][a-z0-9.-]*$/i.test(trimmed)) {
    candidate = `https://${trimmed}`;
  } else {
    return {
      kind: "invalid",
      reason: "That doesn't look like a workspace. Try `yourcompany`.",
    };
  }

  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    return {
      kind: "invalid",
      reason: "That doesn't look like a URL we can reach.",
    };
  }

  if (parsed.protocol !== "https:") {
    return {
      kind: "invalid",
      reason:
        "Atlassian Cloud is https-only. Remove `http://` (or switch to `https://`).",
    };
  }

  if (parsed.hostname.length === 0) {
    return {
      kind: "invalid",
      reason: "Missing host. Example: yourcompany.atlassian.net",
    };
  }

  // DOG-v0.2-03 (security). Reject any host outside Atlassian
  // Cloud's `.atlassian.net` apex. Without this, a paste of
  // `https://attacker.example/` would survive client-side
  // normalisation; the IPC layer rejects the same input as a
  // belt-and-braces second check (`parse_workspace_url`), but
  // catching it here keeps the `BasicAuth` build inside the dialog
  // from posting the API token to the wrong origin during the
  // pre-submit `atlassian_validate_credentials` round-trip.
  const hostLower = parsed.hostname.toLowerCase();
  const hostOk =
    hostLower === "atlassian.net" || hostLower.endsWith(".atlassian.net");
  if (!hostOk) {
    return {
      kind: "invalid",
      reason:
        "Workspace URL must be a `*.atlassian.net` Atlassian Cloud tenant.",
    };
  }

  // Path components other than `/` are not part of the workspace URL
  // — the Jira and Confluence REST roots (`/rest/api/3/...`,
  // `/wiki/api/v2/...`) are appended by the connector. Silently
  // trimming a path would let `/wiki` sneak in and collide with the
  // connector's own path assembly; reject loudly.
  if (parsed.pathname !== "/" && parsed.pathname !== "") {
    return {
      kind: "invalid",
      reason:
        "Workspace URL must be origin-only. Remove the path segment (e.g. `/wiki`).",
    };
  }

  if (parsed.search.length > 0 || parsed.hash.length > 0) {
    return {
      kind: "invalid",
      reason: "Workspace URL must not include a query string or fragment.",
    };
  }

  // `URL.origin` already strips the trailing slash, which is the
  // shape the IPC layer stores on
  // `SourceConfig::{Jira, Confluence}.workspace_url`.
  return { kind: "ok", url: parsed.origin };
}

/**
 * Build the Atlassian API-token management page URL. Reused by the
 * "Open token page" button in `AddAtlassianSourceDialog`. The page
 * is account-scoped (lives at `id.atlassian.com`), not workspace-
 * scoped, so the workspace URL is not part of the path.
 */
export function atlassianTokenPageUrl(): string {
  return "https://id.atlassian.com/manage-profile/security/api-tokens";
}
