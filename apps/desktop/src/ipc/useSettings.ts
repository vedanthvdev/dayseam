// DAY-149. React binding for the `settings_get` / `settings_update`
// IPC pair.
//
// Until DAY-149 the frontend did not read `Settings` directly — the
// only persisted field the UI cared about was `theme`, and that path
// is handled by `ThemeProvider` through `localStorage` plus a native
// View menu event. Adding the `keep_running_when_window_closed`
// toggle forces us to actually round-trip the `Settings` blob so the
// Preferences dialog can show the current value and write a patch
// back. `verbose_logs` hitches a ride on the same hook — even though
// no UI reads it today, keeping all `Settings` fields flowing through
// one place means the next field we add lands with zero new wiring.
//
// The shape deliberately matches `useSinks` / `useSources` so
// reviewers don't have to context-switch between "how does every
// other settings-ish hook work" and this one: `loading` for the
// initial fetch, `error` for transport or deserialisation failures,
// `refresh` for manual re-reads (currently only tests call it), and
// `save` for applying a partial patch. `save` accepts a
// `SettingsPatch` so callers never have to reason about which fields
// they're changing vs keeping — the Rust side's `Settings::with_patch`
// merges atomically and re-stamps `config_version`, which is exactly
// the "partial update" behaviour React forms want.

import { useCallback, useEffect, useState } from "react";
import type { Settings, SettingsPatch } from "@dayseam/ipc-types";
import { invoke } from "./invoke";

export interface UseSettingsState {
  settings: Settings | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  save: (patch: SettingsPatch) => Promise<Settings>;
}

export function useSettings(): UseSettingsState {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const row = await invoke("settings_get", {});
      setSettings(row);
    } catch (err) {
      setError(err instanceof Error ? err.message : JSON.stringify(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = useCallback(async (patch: SettingsPatch) => {
    // Intentionally do NOT toggle `loading` during a save — the
    // Preferences dialog keeps its own disabled-button state, and
    // flipping `loading: true` here would also blank any
    // settings-reading component for the duration of the round
    // trip.
    const next = await invoke("settings_update", { patch });
    setSettings(next);
    return next;
  }, []);

  return { settings, loading, error, refresh, save };
}
