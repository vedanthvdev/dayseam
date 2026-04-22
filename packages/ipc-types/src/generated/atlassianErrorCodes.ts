// AUTO-GENERATED FILE. Do not edit by hand.
// Regenerated from `dayseam_core::error_codes::ALL` by the
// `ts_types_generated` test. Includes every `atlassian.*`,
// `jira.*`, and `confluence.*` code — the three-prefix split
// the Atlassian stack uses. Add the copy entry in
// `src/features/sources/atlassianErrorCopy.ts` whenever this
// list grows, otherwise the frontend parity test fails.

export const ATLASSIAN_ERROR_CODES = [
  "atlassian.auth.invalid_credentials",
  "atlassian.auth.missing_scope",
  "atlassian.cloud.resource_not_found",
  "atlassian.identity.malformed_account_id",
  "atlassian.adf.unrenderable_node",
  "jira.walk.upstream_shape_changed",
  "jira.walk.rate_limited",
  "jira.upstream_5xx",
  "jira.resource_gone",
  "confluence.walk.upstream_shape_changed",
  "confluence.walk.rate_limited",
  "confluence.upstream_5xx",
  "confluence.resource_gone",
] as const;

export type AtlassianErrorCode = (typeof ATLASSIAN_ERROR_CODES)[number];
