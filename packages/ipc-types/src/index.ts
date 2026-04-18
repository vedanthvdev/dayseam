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

export type { Identity } from "./generated/Identity";
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

export type { DayseamError } from "./generated/DayseamError";

export type { JsonValue } from "./generated/serde_json/JsonValue";
