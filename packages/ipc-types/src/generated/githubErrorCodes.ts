// AUTO-GENERATED FILE. Do not edit by hand.
// Regenerated from `dayseam_core::error_codes::ALL` by the
// `ts_types_generated` test. Add the copy entry in
// `src/features/sources/githubErrorCopy.ts` whenever this list
// grows, otherwise the frontend parity test fails.

export const GITHUB_ERROR_CODES = [
  "github.auth.invalid_credentials",
  "github.auth.missing_scope",
  "github.resource_not_found",
  "github.rate_limited",
  "github.upstream_5xx",
  "github.upstream_shape_changed",
  "github.resource_gone",
] as const;

export type GithubErrorCode = (typeof GITHUB_ERROR_CODES)[number];
