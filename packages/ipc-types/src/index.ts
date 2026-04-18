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

export type { Source } from "./generated/Source";
export type { SourceConfig } from "./generated/SourceConfig";
export type { SourceHealth } from "./generated/SourceHealth";
export type { SourceKind } from "./generated/SourceKind";
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
