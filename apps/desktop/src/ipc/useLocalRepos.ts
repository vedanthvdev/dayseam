// React binding for the `local_repos_*` IPC surface.
//
// Local repos are the rows created by `sources_add` for a
// `LocalGit` source: the Rust side walks the configured scan roots,
// inserts one row per `.git` directory it finds, and marks them
// public by default. The UI's only live operation is flipping a
// repo's `is_private` flag; the discovery pass runs once at
// `sources_add` time (and again on `sources_update` when the scan
// roots change) and the hook doesn't expose a "rediscover now"
// command in this phase.

import { useCallback, useEffect, useState } from "react";
import type { LocalRepo } from "@dayseam/ipc-types";
import { invoke } from "./invoke";
import { SOURCES_CHANGED, sourcesBus } from "./useSources";

export interface UseLocalReposState {
  repos: LocalRepo[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  setPrivate: (path: string, isPrivate: boolean) => Promise<LocalRepo>;
}

export function useLocalRepos(sourceId: string | null): UseLocalReposState {
  const [repos, setRepos] = useState<LocalRepo[]>([]);
  const [loading, setLoading] = useState(sourceId !== null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (sourceId === null) {
      setRepos([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const rows = await invoke("local_repos_list", { sourceId });
      setRepos(rows);
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    } finally {
      setLoading(false);
    }
  }, [sourceId]);

  useEffect(() => {
    // Initial fetch plus a subscription to the sources bus. Discovery
    // re-runs on `sources_add` / `sources_update`, so any consumer
    // rendering a repo count for a live source needs to re-query
    // after the sources-side notification lands. Healthcheck also
    // pings this but the resulting extra fetch is a cheap local
    // SQLite read, so we don't bother filtering event types.
    void refresh();
    if (sourceId === null) return;
    const handler = () => {
      void refresh();
    };
    sourcesBus.addEventListener(SOURCES_CHANGED, handler);
    return () => sourcesBus.removeEventListener(SOURCES_CHANGED, handler);
  }, [refresh, sourceId]);

  const setPrivate = useCallback(
    async (path: string, isPrivate: boolean) => {
      const updated = await invoke("local_repos_set_private", {
        path,
        isPrivate,
      });
      await refresh();
      return updated;
    },
    [refresh],
  );

  return { repos, loading, error, refresh, setPrivate };
}
