// Normalisation for the GitLab base-URL field in
// `AddGitlabSourceDialog`.
//
// Design §6.2.1 says the stored `SourceConfig::GitLab.base_url` is the
// origin the connector appends `/api/v4/...` to — so it must be a bare
// scheme://host(:port) with no trailing slash and no path components.
// The user's input is looser: they'll paste whatever is in their
// browser bar, occasionally without a scheme and occasionally with a
// trailing slash. This module is the single place that loose input is
// turned into the tight stored shape, so both the dialog and its tests
// can consume the same six cases without duplicating the tolerance
// rules.
//
// Intentionally **not** silently upgraded to HTTPS (per plan §3
// invariant "http:// is downgraded loudly, not silently upgraded"): if
// the user typed `http://`, we keep `http://` and surface a warning
// from the caller so they see that a cleartext PAT exchange is about
// to happen. Design §13's TLS posture is "the user gets to choose,
// but we never silently un-choose their http:// to cover for it".

/** Outcome of normalising whatever the user typed into the URL box. */
export type BaseUrlNormalisation =
  | { kind: "ok"; url: string; insecure: boolean }
  | { kind: "empty" }
  | { kind: "invalid"; reason: string };

const ALLOWED_SCHEMES = new Set(["http:", "https:"]);

/**
 * Turn raw user input into a normalised `SourceConfig::GitLab.base_url`.
 *
 * Six acceptance cases (per Task 3 plan, `base_url_normalisation_table`):
 *
 * 1. `gitlab.example.com`          → `https://gitlab.example.com`   (scheme prefilled)
 * 2. `https://gitlab.example.com`  → `https://gitlab.example.com`   (identity)
 * 3. `http://gitlab.example.com`   → `http://gitlab.example.com`    (kept; `insecure: true`)
 * 4. `gitlab.example.com/`         → `https://gitlab.example.com`   (trailing slash stripped)
 * 5. `gitlab.example.com/path`     → `invalid`                      (path not allowed)
 * 6. ``                            → `empty`                        (submit stays disabled)
 */
export function normaliseBaseUrl(raw: string): BaseUrlNormalisation {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return { kind: "empty" };

  // Default to https:// if no scheme is present. `new URL` requires a
  // scheme or it throws, so we paste one on the front of scheme-less
  // input and mark it as not-insecure for the caller.
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

  if (!ALLOWED_SCHEMES.has(parsed.protocol)) {
    return {
      kind: "invalid",
      reason: `Scheme ${parsed.protocol} is not supported. Use http:// or https://.`,
    };
  }

  if (parsed.hostname.length === 0) {
    return {
      kind: "invalid",
      reason: "Missing host. Example: gitlab.example.com",
    };
  }

  // Path components other than `/` are not part of the base URL —
  // GitLab self-hosted instances live at the origin, and appending a
  // user-supplied path segment would silently break `/api/v4/...`
  // assembly downstream. Reject loudly rather than silently trim.
  if (parsed.pathname !== "/" && parsed.pathname !== "") {
    return {
      kind: "invalid",
      reason:
        "Base URL must be the origin only. Remove the path segment and any trailing slug.",
    };
  }

  if (parsed.search.length > 0 || parsed.hash.length > 0) {
    return {
      kind: "invalid",
      reason: "Base URL must not include a query string or fragment.",
    };
  }

  // `URL.origin` already strips the trailing slash for us, so this is
  // the shape we store on `SourceConfig::GitLab.base_url`.
  return {
    kind: "ok",
    url: parsed.origin,
    insecure: parsed.protocol === "http:",
  };
}

/** Build the Personal Access Tokens page URL for a normalised host.
 *  Matches the path called out in design §6.2.1 and referenced by the
 *  "Open token page" button in `AddGitlabSourceDialog`. */
export function tokenPageUrl(baseUrl: string): string {
  return `${baseUrl.replace(/\/$/, "")}/-/user_settings/personal_access_tokens?name=Dayseam&scopes=read_api,read_user`;
}
