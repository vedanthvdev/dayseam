// React binding for the `sources_*` IPC surface.
//
// The hook owns the list of configured [`Source`]s plus the four
// mutating operations the Phase 2 UI needs — add, update, delete,
// and healthcheck. Every successful mutation notifies a module-level
// bus so every other live `useSources()` instance re-fetches; that
// keeps the sources row in the action bar and the sources strip in
// the title area in sync after an edit or delete. Without the bus
// each consumer has its own local `useState` list and they drift
// silently (the mutator refreshes itself, but nobody else does).
//
// We dispatch on `add` / `update` / `remove` / `healthcheck`. The
// mutator receives the bus event like everyone else, so its own
// refresh happens through the shared listener — no special-casing
// "self". One mutation produces N refreshes where N is the number of
// mounted hook instances; N is small (< a dozen in practice) and
// `sources_list` is a cheap local SQLite query, so we eat the cost
// for the simplicity win.
//
// Mutations surface their errors by rethrowing so the caller can
// show them inline (e.g. on the form that initiated the call) while
// keeping the list state itself free of transient mutation errors.

import { useCallback, useEffect, useState } from "react";
import type {
  Source,
  SourceConfig,
  SourceHealth,
  SourceKind,
  SourcePatch,
} from "@dayseam/ipc-types";
import { invoke } from "./invoke";

export interface UseSourcesState {
  sources: Source[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  add: (
    kind: SourceKind,
    label: string,
    config: SourceConfig,
  ) => Promise<Source>;
  update: (id: string, patch: SourcePatch) => Promise<Source>;
  remove: (id: string) => Promise<void>;
  healthcheck: (id: string) => Promise<SourceHealth>;
}

// Module-level fan-out. `EventTarget` is available in the browser
// and jsdom, so tests don't need a polyfill.
//
// Exported so peer hooks (currently `useLocalRepos`) can subscribe
// too — `sources_add` / `sources_update` can discover new repos
// under the scan roots, and the sidebar chips surface a repo count
// that must stay in sync with that discovery. Exposing the bus is
// cheaper than duplicating the fan-out or adding a second bus.
export const sourcesBus = new EventTarget();
export const SOURCES_CHANGED = "sources:changed";

function notifySourcesChanged(): void {
  sourcesBus.dispatchEvent(new Event(SOURCES_CHANGED));
}

export function useSources(): UseSourcesState {
  const [sources, setSources] = useState<Source[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const rows = await invoke("sources_list", {});
      setSources(rows);
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // Initial fetch AND subscription to cross-instance changes. Both
    // happen in the same effect so we only register the listener
    // once per hook mount and guarantee cleanup on unmount.
    void refresh();
    const handler = () => {
      void refresh();
    };
    sourcesBus.addEventListener(SOURCES_CHANGED, handler);
    return () => sourcesBus.removeEventListener(SOURCES_CHANGED, handler);
  }, [refresh]);

  const add = useCallback(
    async (kind: SourceKind, label: string, config: SourceConfig) => {
      const source = await invoke("sources_add", { kind, label, config });
      notifySourcesChanged();
      return source;
    },
    [],
  );

  const update = useCallback(async (id: string, patch: SourcePatch) => {
    const source = await invoke("sources_update", { id, patch });
    notifySourcesChanged();
    return source;
  }, []);

  const remove = useCallback(async (id: string) => {
    await invoke("sources_delete", { id });
    notifySourcesChanged();
  }, []);

  const healthcheck = useCallback(async (id: string) => {
    const health = await invoke("sources_healthcheck", { id });
    notifySourcesChanged();
    return health;
  }, []);

  return { sources, loading, error, refresh, add, update, remove, healthcheck };
}
