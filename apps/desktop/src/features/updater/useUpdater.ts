// React binding for the Tauri v2 updater plugin.
//
// The hook drives the full check → download → verify → install →
// relaunch lifecycle from a single instance. It's mounted once at
// the app shell so there's exactly one `check()` in flight per app
// session.
//
// Design choices worth the read:
//
//   - `check()` runs once on mount. No periodic polling — the user
//     restarts the app often enough, and a background poll would
//     just be extra IPC noise plus a fresh "update available"
//     banner that outran whatever the user was doing.
//
//   - `downloadAndInstall()` is the single privileged operation.
//     We don't expose the split `download()` / `install()` pair
//     (the capability file in `capabilities/updater.json` refuses
//     those specifically) because there's no UX benefit to a
//     paused mid-download state and a narrower grant is cheaper
//     to audit.
//
//   - Progress events are bucketed into `percent` only when the
//     server sent `Content-Length`. If it didn't (rare on a raw
//     GitHub asset but possible through a proxy), we fall back to
//     an indeterminate "downloading" state instead of faking a
//     percentage off wall-clock time.
//
//   - After `install()` resolves we always call `relaunch()`. On
//     macOS that's required (the `.app` is swapped in place but
//     the running process keeps the old binary mapped); on
//     Windows/Linux the plugin already restarts the app and
//     `relaunch()` becomes a fast-path no-op.

import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { listen } from "@tauri-apps/api/event";
import { isSkipped, skipVersion } from "./skipped-versions";

export type UpdaterStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | {
      kind: "available";
      version: string;
      currentVersion: string;
      notes: string | null;
    }
  | {
      kind: "downloading";
      version: string;
      percent: number | null;
    }
  | { kind: "ready"; version: string }
  | { kind: "error"; message: string };

export interface UpdaterState {
  status: UpdaterStatus;
  /** Re-run the update check. Safe to call from any state. */
  check: () => Promise<void>;
  /** Download + install the currently-available update, then
   *  relaunch the app. No-op unless `status.kind === "available"`. */
  install: () => Promise<void>;
  /** Persist a skip for the currently-available version and dismiss
   *  the banner for the rest of this session. No-op in any other
   *  state. */
  skipCurrent: () => void;
  /** Whether the currently-available version is on the skip list.
   *  Tests (and the banner) read this to gate rendering. */
  isCurrentSkipped: boolean;
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Unknown error";
}

export function useUpdater(): UpdaterState {
  const [status, setStatus] = useState<UpdaterStatus>({ kind: "idle" });
  // Cache the `Update` resource so `install()` can reuse the handle
  // `check()` returned. `Update` extends `Resource` on the Rust
  // side and must be closed exactly once; we close it in the
  // post-install cleanup branch.
  const updateRef = useRef<Update | null>(null);
  // Tracks whether the component is still mounted so we don't
  // `setState` after unmount if `check()` or the download stream
  // resolve late (e.g. slow network → user quits mid-download).
  const mountedRef = useRef(true);

  const setStatusIfMounted = useCallback((next: UpdaterStatus) => {
    if (mountedRef.current) setStatus(next);
  }, []);

  // Shared helper for releasing whatever `updateRef.current` holds
  // and clearing the slot. Factored out so every branch that
  // overwrites the ref — null resolution, fresh-handle replacement,
  // unmount teardown — goes through the same code path. `close()`
  // on an already-released handle is harmless, but calling it
  // reliably is what keeps the C-5 bridge-handle leak shut.
  const releaseHandle = useCallback(() => {
    const held = updateRef.current;
    updateRef.current = null;
    if (held) {
      void held.close().catch(() => {
        // already-released handle; no-op
      });
    }
  }, []);

  const runCheck = useCallback(async () => {
    setStatusIfMounted({ kind: "checking" });
    try {
      const update = await check();
      if (!update) {
        // DAY-122 / C-5. A prior `check()` may have resolved to an
        // `Update` and stashed the Tauri `Resource` in
        // `updateRef.current` — a later re-check (e.g. the native
        // "Check for Updates…" menu item, or the post-install
        // relaunch path that never fires on relaunch failure)
        // that resolves to `null` used to silently overwrite the
        // ref and leak the prior handle on the Rust side. Close
        // the stale handle *before* declaring up-to-date so the
        // resource slot matches the UI state.
        releaseHandle();
        setStatusIfMounted({ kind: "up-to-date" });
        return;
      }
      // Close any stale handle from a prior check before replacing
      // it — the Rust resource slot is the source of truth here.
      if (updateRef.current && updateRef.current !== update) {
        void updateRef.current.close().catch(() => {
          // A double-close from an already-released handle is
          // harmless; swallow to keep the UI quiet.
        });
      }
      updateRef.current = update;
      setStatusIfMounted({
        kind: "available",
        version: update.version,
        currentVersion: update.currentVersion,
        notes: update.body ?? null,
      });
    } catch (err) {
      setStatusIfMounted({ kind: "error", message: formatError(err) });
    }
  }, [setStatusIfMounted, releaseHandle]);

  const install = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;
    const version = update.version;
    setStatusIfMounted({ kind: "downloading", version, percent: null });
    let total: number | null = null;
    let received = 0;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? null;
          received = 0;
          setStatusIfMounted({
            kind: "downloading",
            version,
            percent: total ? 0 : null,
          });
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          const percent = total
            ? Math.min(100, Math.round((received / total) * 100))
            : null;
          setStatusIfMounted({ kind: "downloading", version, percent });
        } else if (event.event === "Finished") {
          setStatusIfMounted({ kind: "ready", version });
        }
      });
      // On macOS the `.app` swap is complete but the running
      // process still has the old binary mapped, so we must
      // relaunch explicitly. On Windows/Linux the plugin already
      // restarted the app by the time we get here and this call
      // is a no-op.
      await relaunch();
    } catch (err) {
      setStatusIfMounted({ kind: "error", message: formatError(err) });
    }
  }, [setStatusIfMounted]);

  const skipCurrent = useCallback(() => {
    if (status.kind !== "available") return;
    skipVersion(status.version);
    setStatusIfMounted({ kind: "up-to-date" });
  }, [status, setStatusIfMounted]);

  // Fire a single background check when the hook mounts. Errors
  // here land in `status = error` rather than bubbling — the app
  // shell stays fully functional if GitHub is unreachable.
  useEffect(() => {
    mountedRef.current = true;
    void runCheck();
    return () => {
      mountedRef.current = false;
      // Release the underlying Tauri `Resource` so it doesn't
      // leak past the component lifecycle. `close()` is safe to
      // call even if the resource was never downloaded.
      releaseHandle();
    };
    // `runCheck` and `releaseHandle` are stable (useCallback has
    // no deps that change) so this effect really does run exactly
    // once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // DAY-119: listen for the native "Check for Updates…" menu item
  // (installed by the Rust setup hook). The menu emits
  // `menu://check-for-updates` so the JS state machine stays the
  // single source of truth for updater status — the menu action
  // just drives the same `runCheck()` path the mount-time check
  // uses. If the event API is unavailable (test harness, browser
  // fallback) we simply skip registering; the mount-time check
  // still runs.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void listen("menu://check-for-updates", () => {
      void runCheck();
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {
        // No Tauri event bridge (e.g. under vitest/jsdom) — the
        // automatic mount-time check still runs, so the rest of
        // the updater flow remains testable.
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [runCheck]);

  const isCurrentSkipped =
    status.kind === "available" ? isSkipped(status.version) : false;

  return { status, check: runCheck, install, skipCurrent, isCurrentSkipped };
}
