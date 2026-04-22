// Single public entry point for `@dayseam/ipc-types`.
//
// Every type re-exported here is generated from Rust by `ts-rs` and lives
// under `./generated/`. The frontend should only ever import from the
// package root (e.g. `import type { ActivityEvent } from "@dayseam/ipc-types"`)
// so we keep one stable surface even if the underlying file layout shifts.

export type { ActivityEvent } from "./generated/ActivityEvent";
export type { ActivityKind } from "./generated/ActivityKind";
export type { Actor } from "./generated/Actor";
export type { EntityRef } from "./generated/EntityRef";
export type { Link } from "./generated/Link";
export type { Privacy } from "./generated/Privacy";
export type { RawRef } from "./generated/RawRef";

export type { Artifact } from "./generated/Artifact";
export type { ArtifactId } from "./generated/ArtifactId";
export type { ArtifactKind } from "./generated/ArtifactKind";
export type { ArtifactPayload } from "./generated/ArtifactPayload";

export type { SyncRun } from "./generated/SyncRun";
export type { SyncRunTrigger } from "./generated/SyncRunTrigger";
export type { SyncRunStatus } from "./generated/SyncRunStatus";
export type { SyncRunCancelReason } from "./generated/SyncRunCancelReason";
export type { PerSourceState } from "./generated/PerSourceState";

export type { Source } from "./generated/Source";
export type { SourceConfig } from "./generated/SourceConfig";
export type { SourceHealth } from "./generated/SourceHealth";
export type { SourceKind } from "./generated/SourceKind";
export type { SourcePatch } from "./generated/SourcePatch";
export type { SecretRef } from "./generated/SecretRef";
export type { GitlabValidationResult } from "./generated/GitlabValidationResult";
export type { AtlassianValidationResult } from "./generated/AtlassianValidationResult";
export type { GithubValidationResult } from "./generated/GithubValidationResult";
export {
  GITLAB_ERROR_CODES,
  type GitlabErrorCode,
} from "./generated/gitlabErrorCodes";
export {
  ATLASSIAN_ERROR_CODES,
  type AtlassianErrorCode,
} from "./generated/atlassianErrorCodes";
export {
  GITHUB_ERROR_CODES,
  type GithubErrorCode,
} from "./generated/githubErrorCodes";

export type { Sink } from "./generated/Sink";
export type { SinkCapabilities } from "./generated/SinkCapabilities";
export type { SinkConfig } from "./generated/SinkConfig";
export type { SinkKind } from "./generated/SinkKind";
export type { WriteReceipt } from "./generated/WriteReceipt";

export type { Identity } from "./generated/Identity";
export type { Person } from "./generated/Person";
export type { SourceIdentity } from "./generated/SourceIdentity";
export type { SourceIdentityKind } from "./generated/SourceIdentityKind";
export type { LocalRepo } from "./generated/LocalRepo";

export type { ReportDraft } from "./generated/ReportDraft";
export type { RenderedSection } from "./generated/RenderedSection";
export type { RenderedBullet } from "./generated/RenderedBullet";
export type { Evidence } from "./generated/Evidence";
export type { SourceRunState } from "./generated/SourceRunState";
export type { RunStatus } from "./generated/RunStatus";
export type { LogEntry } from "./generated/LogEntry";
export type { LogLevel } from "./generated/LogLevel";

export type { RunId } from "./generated/RunId";
export type { ProgressEvent } from "./generated/ProgressEvent";
export type { ProgressPhase } from "./generated/ProgressPhase";
export type { LogEvent } from "./generated/LogEvent";
export type { ToastEvent } from "./generated/ToastEvent";
export type { ToastSeverity } from "./generated/ToastSeverity";
export type { ReportCompletedEvent } from "./generated/ReportCompletedEvent";

export type { Settings } from "./generated/Settings";
export type { SettingsPatch } from "./generated/SettingsPatch";
export type { ThemePreference } from "./generated/ThemePreference";

export type { DayseamError } from "./generated/DayseamError";

export type { JsonValue } from "./generated/serde_json/JsonValue";

// ----- Tauri command catalog -----
//
// Single source of truth for the Rust `#[tauri::command]` surface
// exposed through `#[tauri::command]` and allow-listed in
// `apps/desktop/src-tauri/capabilities/default.json`. The frontend's
// typed `invoke(name, args)` helper reads its type parameters off of
// this map — adding or renaming a Rust command without touching this
// union is a compile-error on the TS side.
//
// Each entry is `{ args, result }`:
//   - `args` is the object Tauri receives as the command payload
//     (the argument names/shapes must match the Rust signature after
//     snake_case → camelCase on the Rust side, if configured — we
//     keep snake_case end-to-end for IPC for simplicity).
//   - `result` is the resolved value of the returned `Promise`.
//
// Commands that stream via `Channel<T>` (e.g. `dev_start_demo_run`)
// take the channels by reference in the `args` map; the TS side builds
// them via `@tauri-apps/api/core::Channel`.

import type { LogEntry } from "./generated/LogEntry";
import type { ProgressEvent } from "./generated/ProgressEvent";
import type { LogEvent } from "./generated/LogEvent";
import type { ToastEvent } from "./generated/ToastEvent";
import type { RunId } from "./generated/RunId";
import type { Settings as SettingsT } from "./generated/Settings";
import type { SettingsPatch as SettingsPatchT } from "./generated/SettingsPatch";
import type { Person as PersonT } from "./generated/Person";
import type { Source as SourceT } from "./generated/Source";
import type { SourceConfig as SourceConfigT } from "./generated/SourceConfig";
import type { SourceKind as SourceKindT } from "./generated/SourceKind";
import type { SourceHealth as SourceHealthT } from "./generated/SourceHealth";
import type { SourcePatch as SourcePatchT } from "./generated/SourcePatch";
import type { SourceIdentity as SourceIdentityT } from "./generated/SourceIdentity";
import type { LocalRepo as LocalRepoT } from "./generated/LocalRepo";
import type { Sink as SinkT } from "./generated/Sink";
import type { SinkConfig as SinkConfigT } from "./generated/SinkConfig";
import type { SinkKind as SinkKindT } from "./generated/SinkKind";
import type { ReportDraft as ReportDraftT } from "./generated/ReportDraft";
import type { WriteReceipt as WriteReceiptT } from "./generated/WriteReceipt";
import type { ActivityEvent as ActivityEventT } from "./generated/ActivityEvent";
import type { GitlabValidationResult as GitlabValidationResultT } from "./generated/GitlabValidationResult";
import type { AtlassianValidationResult as AtlassianValidationResultT } from "./generated/AtlassianValidationResult";
import type { GithubValidationResult as GithubValidationResultT } from "./generated/GithubValidationResult";
import type { SecretRef as SecretRefT } from "./generated/SecretRef";

/** Opaque handle used at the TS boundary for Tauri's `Channel<T>`. */
export interface TauriChannel<T> {
  onmessage?: (event: T) => void;
}

export interface Commands {
  settings_get: {
    args: Record<string, never>;
    result: SettingsT;
  };
  settings_update: {
    args: { patch: SettingsPatchT };
    result: SettingsT;
  };
  logs_tail: {
    args: { since: string | null; limit: number | null };
    result: LogEntry[];
  };
  persons_get_self: {
    args: Record<string, never>;
    result: PersonT;
  };
  persons_update_self: {
    args: { displayName: string };
    result: PersonT;
  };
  sources_list: {
    args: Record<string, never>;
    result: SourceT[];
  };
  sources_add: {
    // DAY-70: `pat` is the Personal Access Token for GitLab sources.
    // Required (non-empty) when `kind === "GitLab"`; ignored for
    // LocalGit. Typed as `string` rather than a branded secret type
    // because Tauri IPC serialises it as a plain JSON string, and
    // the Rust side wraps the inbound value in `IpcSecretString` (a
    // `ZeroizeOnDrop` wrapper that never implements `Serialize`)
    // before anything else can observe it.
    args: {
      kind: SourceKindT;
      label: string;
      config: SourceConfigT;
      pat: string | null;
    };
    result: SourceT;
  };
  sources_update: {
    // DAY-70: `pat` lets the Reconnect flow rotate the stored GitLab
    // token in the same round-trip as a config/label edit. `null`
    // leaves the keychain entry untouched; a non-empty string
    // overwrites it.
    args: { id: string; patch: SourcePatchT; pat: string | null };
    result: SourceT;
  };
  sources_delete: {
    args: { id: string };
    result: null;
  };
  sources_healthcheck: {
    args: { id: string };
    result: SourceHealthT;
  };
  identities_list_for: {
    args: { personId: string };
    result: SourceIdentityT[];
  };
  identities_upsert: {
    args: { identity: SourceIdentityT };
    result: SourceIdentityT;
  };
  identities_delete: {
    args: { id: string };
    result: null;
  };
  local_repos_list: {
    args: { sourceId: string };
    result: LocalRepoT[];
  };
  local_repos_set_private: {
    args: { path: string; isPrivate: boolean };
    result: LocalRepoT;
  };
  sinks_list: {
    args: Record<string, never>;
    result: SinkT[];
  };
  sinks_add: {
    args: { kind: SinkKindT; label: string; config: SinkConfigT };
    result: SinkT;
  };
  report_generate: {
    args: {
      date: string;
      sourceIds: string[];
      templateId: string | null;
      progress: TauriChannel<ProgressEvent>;
      logs: TauriChannel<LogEvent>;
    };
    result: RunId;
  };
  report_cancel: {
    args: { runId: RunId };
    result: null;
  };
  report_get: {
    args: { draftId: string };
    result: ReportDraftT;
  };
  report_save: {
    args: { draftId: string; sinkId: string };
    result: WriteReceiptT[];
  };
  retention_sweep_now: {
    args: Record<string, never>;
    result: null;
  };
  activity_events_get: {
    args: { ids: string[] };
    result: ActivityEventT[];
  };
  shell_open: {
    args: { url: string };
    result: null;
  };
  /** One-shot PAT probe. `pat` is transported as a raw JSON string
   *  and wrapped on the Rust side in an `IpcSecretString` that
   *  redacts in `Debug` output and zeroes its bytes on drop — see
   *  `apps/desktop/src-tauri/src/ipc/secret.rs`. The renderer must
   *  never log the PAT (the generated `invoke()` helper does not
   *  instrument args, but a bespoke wrapper in a feature branch
   *  must preserve that property). */
  gitlab_validate_pat: {
    args: { host: string; pat: string };
    result: GitlabValidationResultT;
  };
  /** One-shot Atlassian credential probe. Calls
   *  `GET /rest/api/3/myself` with the email + API token over Basic
   *  auth and, on success, returns the account triple the add-source
   *  dialog renders in its "Connected as …" confirmation ribbon.
   *  `apiToken` is transported as a raw JSON string and wrapped on
   *  the Rust side in an `IpcSecretString` that redacts in `Debug`
   *  output and zeroes its bytes on drop. Introduced in DAY-82. */
  atlassian_validate_credentials: {
    args: { workspaceUrl: string; email: string; apiToken: string };
    result: AtlassianValidationResultT;
  };
  /** Persist one or two Atlassian sources in a single round-trip.
   *  Four journeys share this one command:
   *
   *    - Journey A (shared PAT, both products). Both `enableJira`
   *      and `enableConfluence` are `true` and `reuseSecretRef` is
   *      `null`; the command writes one keychain row + two `sources`
   *      rows that share its `secret_ref`.
   *    - Journey B (single product). One of the two enable flags is
   *      `false`; the command writes one keychain row + one `sources`
   *      row.
   *    - Journey C mode 1 (reuse existing PAT for the other product).
   *      `reuseSecretRef` is `Some(_)`; the command writes **no new
   *      keychain row** — it stamps the supplied `secret_ref` onto
   *      the new `sources` row so DAY-81's refcount treats the pair
   *      as shared from the start. `apiToken` MAY be `null` in this
   *      mode — the dialog is one-click and never re-prompts for the
   *      PAT.
   *    - Journey C mode 2 (separate PAT for the other product). Same
   *      shape as Journey B but from the "add another product" entry
   *      point; `reuseSecretRef` is `null` and a distinct keychain
   *      row is written.
   *
   *  `accountId` is the opaque `/rest/api/3/myself` account id
   *  `atlassian_validate_credentials` returned in Journeys A / B /
   *  C-mode-2, or (in Journey C mode 1) the account id the dialog
   *  pulled off the existing source's `AtlassianAccountId` identity
   *  row. The command stamps it onto a fresh `SourceIdentity` per
   *  new source so the render-stage self-filter recognises events
   *  this user authored. Returns the freshly-inserted `Source` rows.
   *  Introduced in DAY-82. */
  atlassian_sources_add: {
    args: {
      workspaceUrl: string;
      email: string;
      apiToken: string | null;
      accountId: string;
      enableJira: boolean;
      enableConfluence: boolean;
      reuseSecretRef: SecretRefT | null;
    };
    result: SourceT[];
  };
  /** Rotate the API token on an existing Atlassian source.
   *
   *  Triggered by the Reconnect chip on `SourceErrorCard` when a
   *  Jira or Confluence source's last walk failed with
   *  `atlassian.auth.invalid_credentials`. The backend validates the
   *  new token against the source's stored `(workspace_url, email)`,
   *  asserts the `/rest/api/3/myself` account id still matches the
   *  `AtlassianAccountId` identity already bound to the source (to
   *  prevent silent account-rebinding), and overwrites the keychain
   *  entry at the existing `SecretRef`.
   *
   *  Shared-PAT sources (Journey A: Jira + Confluence pointing at
   *  the same `SecretRef`) are rotated atomically; the returned
   *  array lists every source id whose token was refreshed so the
   *  caller can fire `sources_healthcheck` for each to clear the
   *  red error chips. Introduced in DAY-87. */
  atlassian_sources_reconnect: {
    args: { sourceId: string; apiToken: string };
    result: string[];
  };
  /** One-shot GitHub credential probe. Calls
   *  `GET <apiBaseUrl>/user` with the PAT over bearer auth and, on
   *  success, returns the account triple the add-source dialog
   *  renders in its "Connected as …" confirmation ribbon. `pat` is
   *  transported as a raw JSON string and wrapped on the Rust side
   *  in an `IpcSecretString` that redacts in `Debug` output and
   *  zeroes its bytes on drop. Introduced in DAY-99. */
  github_validate_credentials: {
    args: { apiBaseUrl: string; pat: string };
    result: GithubValidationResultT;
  };
  /** Persist a single GitHub source in one round-trip. `userId` is
   *  the numeric id `github_validate_credentials` returned; `login`
   *  is the handle the same probe returned. The command stamps both
   *  onto fresh `SourceIdentity` rows under `GitHubUserId` (numeric,
   *  filter-time key) and `GitHubLogin` (handle, used to compose
   *  `/users/{login}/events`). Both rows are required — missing
   *  either makes the walker return zero events on every sync, the
   *  silent-failure chain CORR-v0.4-01 caught at the v0.4 capstone.
   *  `pat` is wrapped on the Rust side in an `IpcSecretString`
   *  before anything else can observe it. Returns the freshly-
   *  inserted `Source` row. Introduced in DAY-99; widened with
   *  `login` in DAY-101. */
  github_sources_add: {
    args: {
      apiBaseUrl: string;
      label: string;
      pat: string;
      userId: number;
      login: string;
    };
    result: SourceT;
  };
  /** Rotate the PAT on an existing GitHub source. Triggered by the
   *  Reconnect chip on `SourceErrorCard` when a GitHub source's
   *  last walk failed with `github.auth.invalid_credentials`. The
   *  backend validates the new token against the source's stored
   *  `api_base_url`, asserts the `/user` numeric id still matches
   *  the `GitHubUserId` identity already bound to the source (to
   *  prevent silent account-rebinding), and overwrites the keychain
   *  entry at the existing `SecretRef`. Returns the source id whose
   *  token was refreshed so the caller can fire
   *  `sources_healthcheck` to clear the red error chip. Introduced
   *  in DAY-99. */
  github_sources_reconnect: {
    args: { sourceId: string; pat: string };
    result: string;
  };
  /** Dev-only. Compiled out of release builds via `cfg(feature = "dev-commands")`. */
  dev_emit_toast: {
    args: { event: ToastEvent };
    result: null;
  };
  /** Dev-only. Compiled out of release builds via `cfg(feature = "dev-commands")`. */
  dev_start_demo_run: {
    args: {
      progress: TauriChannel<ProgressEvent>;
      logs: TauriChannel<LogEvent>;
    };
    result: RunId;
  };
}

/** Union of every command name the frontend can invoke. */
export type CommandName = keyof Commands;

/** Every production command identifier, in the same order as the
 * Rust `PROD_COMMANDS` array in
 * `apps/desktop/src-tauri/src/ipc/commands.rs`. Exported as a
 * runtime value so the Vitest parity test can diff it against the
 * TS type `Commands` and against the Tauri capability file. When
 * adding a new command, edit this list and `Commands` above in the
 * same change; the parity test will fail if they drift. */
export const PROD_COMMANDS: readonly CommandName[] = [
  "settings_get",
  "settings_update",
  "logs_tail",
  "persons_get_self",
  "persons_update_self",
  "sources_list",
  "sources_add",
  "sources_update",
  "sources_delete",
  "sources_healthcheck",
  "identities_list_for",
  "identities_upsert",
  "identities_delete",
  "local_repos_list",
  "local_repos_set_private",
  "sinks_list",
  "sinks_add",
  "report_generate",
  "report_cancel",
  "report_get",
  "report_save",
  "retention_sweep_now",
  "activity_events_get",
  "shell_open",
  "gitlab_validate_pat",
  "atlassian_validate_credentials",
  "atlassian_sources_add",
  "atlassian_sources_reconnect",
  "github_validate_credentials",
  "github_sources_add",
  "github_sources_reconnect",
] as const;

/** Dev-only command identifiers. Gated behind the Rust
 * `dev-commands` feature; the Tauri invoke handler excludes them
 * from release builds entirely. Kept in sync with Rust
 * `DEV_COMMANDS`. */
export const DEV_COMMANDS: readonly CommandName[] = [
  "dev_emit_toast",
  "dev_start_demo_run",
] as const;
