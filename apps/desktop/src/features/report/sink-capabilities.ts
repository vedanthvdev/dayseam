// Declarative map of `SinkKind → SinkCapabilities` kept in lockstep
// with the Rust-side `SinkCapabilities::LOCAL_ONLY` constant on
// `sink-markdown-file`. The UI has no IPC endpoint that returns a
// sink's capabilities (they are a property of the *adapter*, not the
// persisted row), so we codify the mapping here.
//
// When a new `SinkKind` is added to `dayseam-core`, the TypeScript
// discriminated-union exhaustiveness check in
// [`capabilitiesForKind`] will stop compiling until the frontend
// mapping is updated — matching the "triple-write" discipline that
// keeps the Rust adapter, the IPC catalogue, and the UI from drifting.

import type { SinkCapabilities, Sink, SinkKind } from "@dayseam/ipc-types";

/** Unreachable marker that turns an unhandled variant into a
 *  compile-time error rather than a silent fallthrough. */
function assertExhaustive(x: never): never {
  throw new Error(`unhandled SinkKind: ${JSON.stringify(x)}`);
}

export function capabilitiesForKind(kind: SinkKind): SinkCapabilities {
  switch (kind) {
    case "MarkdownFile":
      // Mirrors `SinkCapabilities::LOCAL_ONLY` in `dayseam-core`.
      return {
        local_only: true,
        remote_write: false,
        interactive_only: false,
        safe_for_unattended: true,
      };
    default:
      return assertExhaustive(kind);
  }
}

export interface SinkFilterContext {
  /** `true` when the UI is rendering a save picker that runs without
   *  the user in front of it (future scheduled-run UX). `false` for
   *  the Phase-2 interactive SaveReportDialog. Sinks with
   *  `interactive_only = true` are hidden when this flag is `true`. */
  unattended: boolean;
}

/** Filter a list of persisted `Sink` rows down to the ones that the
 *  current save context is allowed to fire. The interactive-save
 *  path in Phase 2 passes `{ unattended: false }` which keeps every
 *  sink; the filter is exercised anyway so Task 8's scheduled runs
 *  can rely on the same code path without a second implementation. */
export function filterSinksForSave(
  sinks: readonly Sink[],
  context: SinkFilterContext,
): Sink[] {
  return sinks.filter((sink) => {
    const caps = capabilitiesForKind(sink.kind);
    if (context.unattended && caps.interactive_only) return false;
    if (context.unattended && !caps.safe_for_unattended) return false;
    return true;
  });
}
