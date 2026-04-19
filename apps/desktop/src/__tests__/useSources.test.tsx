import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Source } from "@dayseam/ipc-types";
import { useSources } from "../ipc/useSources";
import { registerInvokeHandler, resetTauriMocks } from "./tauri-mock";

const SOURCE_A: Source = {
  id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
  kind: "LocalGit",
  label: "work repos",
  config: { LocalGit: { scan_roots: ["/Users/me/code"] } },
  secret_ref: null,
  created_at: "2026-04-17T12:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
};

describe("useSources", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("sources_list", async () => [SOURCE_A]);
  });

  afterEach(() => {
    resetTauriMocks();
  });

  it("lists sources on mount and exposes them as state", async () => {
    const { result } = renderHook(() => useSources());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.sources).toEqual([SOURCE_A]);
    expect(result.current.error).toBeNull();
  });

  it("re-fetches after `add` so callers see the new row", async () => {
    let callCount = 0;
    registerInvokeHandler("sources_list", async () => {
      callCount += 1;
      return callCount === 1 ? [] : [SOURCE_A];
    });
    registerInvokeHandler("sources_add", async () => SOURCE_A);

    const { result } = renderHook(() => useSources());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.sources).toEqual([]);

    await act(async () => {
      await result.current.add("LocalGit", SOURCE_A.label, SOURCE_A.config);
    });
    expect(result.current.sources).toEqual([SOURCE_A]);
  });

  it("propagates mutations across separate hook instances via the bus", async () => {
    // Regression for the Phase 2 bug where the ActionRow's copy of
    // `sources` never updated after the SourcesSidebar deleted a
    // source, because each `useSources()` kept its own local state.
    let callCount = 0;
    registerInvokeHandler("sources_list", async () => {
      callCount += 1;
      // First render of each hook sees the full list; after delete
      // is dispatched the shared bus asks both to refetch, and the
      // list is now empty.
      return callCount <= 2 ? [SOURCE_A] : [];
    });
    registerInvokeHandler("sources_delete", async () => null);

    const observer = renderHook(() => useSources());
    const mutator = renderHook(() => useSources());
    await waitFor(() => expect(observer.result.current.loading).toBe(false));
    await waitFor(() => expect(mutator.result.current.loading).toBe(false));
    expect(observer.result.current.sources).toEqual([SOURCE_A]);
    expect(mutator.result.current.sources).toEqual([SOURCE_A]);

    await act(async () => {
      await mutator.result.current.remove(SOURCE_A.id);
    });

    // The observer instance, which never called `remove` itself,
    // must still reflect the deletion.
    await waitFor(() => expect(observer.result.current.sources).toEqual([]));
    expect(mutator.result.current.sources).toEqual([]);
  });

  it("surfaces errors from `refresh` without clobbering the existing list", async () => {
    const { result } = renderHook(() => useSources());
    await waitFor(() => expect(result.current.loading).toBe(false));
    registerInvokeHandler("sources_list", async () => {
      throw new Error("boom");
    });
    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.error).toMatch(/boom/);
    // We keep the last-known-good list rather than clearing it —
    // one transient fetch failure shouldn't blank the UI.
    expect(result.current.sources).toEqual([SOURCE_A]);
  });
});
