// AUTO-GENERATED FILE. Do not edit by hand.
// Regenerated from `dayseam_core::error_codes::ALL` by the
// `ts_types_generated` test. Add the copy entry in
// `src/features/sources/gitlabErrorCopy.ts` whenever this list
// grows, otherwise the frontend parity test fails.

export const GITLAB_ERROR_CODES = [
  "gitlab.auth.invalid_token",
  "gitlab.auth.missing_scope",
  "gitlab.url.dns",
  "gitlab.url.tls",
  "gitlab.rate_limited",
  "gitlab.upstream_5xx",
  "gitlab.upstream_shape_changed",
  "gitlab.resource_not_found",
  "gitlab.resource_gone",
] as const;

export type GitlabErrorCode = (typeof GITLAB_ERROR_CODES)[number];
