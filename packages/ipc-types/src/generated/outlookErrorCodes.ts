// AUTO-GENERATED FILE. Do not edit by hand.
// Regenerated from `dayseam_core::error_codes::ALL` by the
// `ts_types_generated` test. Includes every `outlook.*` and
// `ipc.outlook.*` code — the two-prefix split DAY-203 uses
// for walker/Graph-probe vs. pre-network IPC failures. Add
// the copy entry in
// `src/features/sources/outlookErrorCopy.ts` whenever this
// list grows, otherwise the frontend parity test fails.

export const OUTLOOK_ERROR_CODES = [
  "ipc.outlook.session_not_found",
  "ipc.outlook.session_not_ready",
  "ipc.outlook.keychain_write_failed",
  "ipc.outlook.tenant_unresolved",
  "ipc.outlook.source_already_exists",
  "outlook.auth.invalid_credentials",
  "outlook.auth.missing_scope",
  "outlook.consent_required",
  "outlook.resource_not_found",
  "outlook.rate_limited",
  "outlook.upstream_5xx",
  "outlook.upstream_shape_changed",
  "outlook.resource_gone",
] as const;

export type OutlookErrorCode = (typeof OUTLOOK_ERROR_CODES)[number];
