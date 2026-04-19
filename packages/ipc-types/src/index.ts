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
  sources_list: {
    args: Record<string, never>;
    result: SourceT[];
  };
  sources_add: {
    args: { kind: SourceKindT; label: string; config: SourceConfigT };
    result: SourceT;
  };
  sources_update: {
    args: { id: string; patch: SourcePatchT };
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
] as const;

/** Dev-only command identifiers. Gated behind the Rust
 * `dev-commands` feature; the Tauri invoke handler excludes them
 * from release builds entirely. Kept in sync with Rust
 * `DEV_COMMANDS`. */
export const DEV_COMMANDS: readonly CommandName[] = [
  "dev_emit_toast",
  "dev_start_demo_run",
] as const;
