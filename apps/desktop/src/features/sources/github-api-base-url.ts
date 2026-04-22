// Normalisation for the GitHub API base-URL field in
// `AddGithubSourceDialog` (DAY-99).
//
// The `SourceConfig::GitHub.api_base_url` string is what the connector
// calls `Url::join("user")` / `Url::join("search/issues")` against, so
// it must:
//
//   • always carry an `https://` scheme (GitHub refuses cleartext —
//     there is no http:// fallback like there is for self-hosted
//     GitLab, so this module rejects non-https loudly rather than
//     warn-and-continue);
//   • always end with a trailing slash so `Url::join` preserves any
//     path prefix (crucial for Enterprise tenants whose API lives
//     under `/api/v3/`);
//   • never include a query string or fragment;
//   • have a non-empty host.
//
// The six cases this accepts:
//
//   1. (empty)                         → `empty`  (submit disabled)
//   2. `api.github.com`                → `https://api.github.com/`
//   3. `https://api.github.com`        → `https://api.github.com/`    (slash added)
//   4. `https://api.github.com/`       → `https://api.github.com/`    (identity)
//   5. `https://ghe.acme.com/api/v3`   → `https://ghe.acme.com/api/v3/` (GHES, slash added)
//   6. `http://…`                      → `invalid` (https required)
//
// Mirrored server-side by `parse_api_base_url` in
// `apps/desktop/src-tauri/src/ipc/github.rs`; the two diverge only in
// that the TS side auto-adds `https://` when the user pastes a bare
// host (case 2 above) — the Rust side assumes the dialog already
// normalised and refuses schemeless input. That asymmetry is
// intentional: the dialog is the one and only caller that needs to
// turn loose user input into a canonical URL, and the Rust guard is
// defence-in-depth for hand-crafted callers.

/** Outcome of normalising whatever the user typed into the URL box. */
export type GithubApiBaseUrlNormalisation =
  | { kind: "ok"; url: string; isCloud: boolean }
  | { kind: "empty" }
  | { kind: "invalid"; reason: string };

/** The default `api_base_url` value for the GitHub cloud tenant.
 *  `AddGithubSourceDialog` prefills this on open so the one-click
 *  "paste a PAT and go" flow does not require the user to type
 *  anything in the URL box. */
export const GITHUB_CLOUD_API_BASE_URL = "https://api.github.com/";

/** Host for the GitHub cloud web UI — used by [`tokenPageUrl`] to
 *  translate `api.github.com` into the `github.com/settings/tokens`
 *  page the "Open token page" button points at. */
const GITHUB_CLOUD_WEB_HOST = "github.com";
const GITHUB_CLOUD_API_HOST = "api.github.com";

/**
 * Turn raw user input into a normalised
 * `SourceConfig::GitHub.api_base_url`.
 */
export function normaliseGithubApiBaseUrl(
  raw: string,
): GithubApiBaseUrlNormalisation {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return { kind: "empty" };

  // Default to https:// if no scheme is present. `new URL` requires
  // a scheme or it throws, so we paste one on the front of
  // scheme-less input.
  const withScheme = /^[a-z][a-z0-9+\-.]*:\/\//i.test(trimmed)
    ? trimmed
    : `https://${trimmed}`;

  let parsed: URL;
  try {
    parsed = new URL(withScheme);
  } catch {
    return {
      kind: "invalid",
      reason: "That doesn't look like a URL we can reach.",
    };
  }

  if (parsed.protocol !== "https:") {
    return {
      kind: "invalid",
      reason: `GitHub requires https://. Got ${parsed.protocol}.`,
    };
  }

  if (parsed.hostname.length === 0) {
    return {
      kind: "invalid",
      reason: "Missing host. Example: api.github.com",
    };
  }

  if (parsed.search.length > 0 || parsed.hash.length > 0) {
    return {
      kind: "invalid",
      reason: "API base URL must not include a query string or fragment.",
    };
  }

  // Build the canonical stored string: scheme + host (+ port) +
  // path, with a guaranteed trailing slash so downstream `Url::join`
  // does not drop the last path segment.
  const base = `${parsed.origin}${parsed.pathname.endsWith("/") ? parsed.pathname : `${parsed.pathname}/`}`;
  const isCloud = parsed.hostname === GITHUB_CLOUD_API_HOST;

  return { kind: "ok", url: base, isCloud };
}

/** Build the Personal Access Tokens page URL the "Open token page"
 *  button shells out to.
 *
 *  For the GitHub cloud tenant (`api.github.com`) this returns
 *  `https://github.com/settings/tokens/new?...`; for an Enterprise
 *  tenant whose API lives at `https://<host>/api/v3/` we drop the
 *  `/api/v3/` suffix and point at `https://<host>/settings/tokens/new`
 *  — which is the shape GHES surfaces the token UI at. Query params
 *  prefill the token's description and scopes so the user lands on a
 *  form pre-configured for Dayseam's read-only footprint.
 */
export function tokenPageUrl(apiBaseUrl: string): string {
  let parsed: URL;
  try {
    parsed = new URL(apiBaseUrl);
  } catch {
    return apiBaseUrl;
  }
  const host =
    parsed.hostname === GITHUB_CLOUD_API_HOST
      ? GITHUB_CLOUD_WEB_HOST
      : parsed.hostname;
  // `repo` covers private-repo read; `read:org` and `read:user` are
  // required so the walker can enumerate repos the user belongs to
  // and attribute events to their GitHub login respectively. Matches
  // the scope list `docs/plan/2026-04-22-v0.4-github-connector.md`
  // §6 spells out.
  const params = new URLSearchParams({
    description: "Dayseam",
    scopes: "repo,read:org,read:user",
  });
  return `https://${host}/settings/tokens/new?${params.toString()}`;
}
