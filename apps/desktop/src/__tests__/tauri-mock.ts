// In-process mock for the subset of `@tauri-apps/api` we use in tests.
// Tests import helpers from here to register invoke handlers, fire
// event-bus messages, and inspect channel writes without ever going
// near a real Tauri webview.

import { vi } from "vitest";

type InvokeHandler = (args: Record<string, unknown>) => unknown | Promise<unknown>;

const invokeHandlers = new Map<string, InvokeHandler>();

export const mockInvoke = vi.fn(
  async (name: string, args: Record<string, unknown> = {}) => {
    const handler = invokeHandlers.get(name);
    if (!handler) {
      throw new Error(`No mock invoke handler registered for "${name}"`);
    }
    return await handler(args);
  },
);

export function registerInvokeHandler(name: string, handler: InvokeHandler): void {
  invokeHandlers.set(name, handler);
}

export function resetInvokeHandlers(): void {
  invokeHandlers.clear();
  mockInvoke.mockClear();
}

// Every `Channel` instance created during a test is kept in a list so
// the test can reach it via `getChannelsForCommand` after calling the
// command.
const createdChannels: MockChannel<unknown>[] = [];

export class MockChannel<T> {
  onmessage?: (event: T) => void;
  id = createdChannels.length;

  constructor(onmessage?: (event: T) => void) {
    this.onmessage = onmessage;
    createdChannels.push(this as MockChannel<unknown>);
  }

  /** Drive a message from the fake Rust side into the channel. */
  deliver(event: T): void {
    this.onmessage?.(event);
  }

  toJSON(): string {
    return `__CHANNEL__:${this.id}`;
  }
}

export function resetChannels(): void {
  createdChannels.length = 0;
}

/** Return the most recent channels created since the last reset. */
export function getCreatedChannels(): MockChannel<unknown>[] {
  return createdChannels.slice();
}

// Tauri event bus: `listen(name, cb)` returns an unlisten; tests call
// `emitEvent(name, payload)` to drive the bus.
type Listener = (event: { payload: unknown }) => void;
const listeners = new Map<string, Set<Listener>>();

export const mockListen = vi.fn(async (name: string, cb: Listener) => {
  let bucket = listeners.get(name);
  if (!bucket) {
    bucket = new Set();
    listeners.set(name, bucket);
  }
  bucket.add(cb);
  return () => {
    bucket?.delete(cb);
  };
});

export function emitEvent(name: string, payload: unknown): void {
  const bucket = listeners.get(name);
  if (!bucket) return;
  bucket.forEach((cb) => cb({ payload }));
}

export function resetEventBus(): void {
  listeners.clear();
  mockListen.mockClear();
}

// `@tauri-apps/plugin-dialog` is stubbed in `setup.ts` to call through
// to `mockDialogOpen`. Tests use `queueDialogOpen(...)` to script the
// next N calls; after the queue drains the mock returns `null` (the
// same thing the real plugin returns when the user cancels the
// picker).
type DialogOpenResponse = string | string[] | null;
const dialogOpenQueue: DialogOpenResponse[] = [];

export const mockDialogOpen = vi.fn(
  async (_options?: Record<string, unknown>): Promise<DialogOpenResponse> => {
    if (dialogOpenQueue.length > 0) {
      return dialogOpenQueue.shift()!;
    }
    return null;
  },
);

export function queueDialogOpen(...responses: DialogOpenResponse[]): void {
  dialogOpenQueue.push(...responses);
}

export function resetDialogPlugin(): void {
  dialogOpenQueue.length = 0;
  mockDialogOpen.mockClear();
}

/** One-call reset used by `beforeEach` in tests. */
export function resetTauriMocks(): void {
  resetInvokeHandlers();
  resetChannels();
  resetEventBus();
  resetDialogPlugin();
}

// ---------------------------------------------------------------------------
// Onboarding fixture
// ---------------------------------------------------------------------------
//
// Every test that renders `<App />` must either finish onboarding (so
// the main layout mounts) or opt into the first-run empty state
// explicitly. Keeping a single fixture here means a new onboarding
// step doesn't force us to chase every App-level test suite.

export const ONBOARDED_PERSON_ID = "11111111-1111-1111-1111-111111111111";
export const ONBOARDED_SOURCE_ID = "22222222-2222-2222-2222-222222222222";
export const ONBOARDED_IDENTITY_ID = "33333333-3333-3333-3333-333333333333";
export const ONBOARDED_SINK_ID = "44444444-4444-4444-4444-444444444444";

/** Register the four `invoke` handlers the setup checklist reads so
 *  it reports `complete: true`. Call from `beforeEach` in any App-
 *  level test whose subject is the main layout rather than the
 *  first-run experience. */
export function registerOnboardingComplete(): void {
  registerInvokeHandler("persons_get_self", async () => ({
    id: ONBOARDED_PERSON_ID,
    display_name: "Vedanth",
    is_self: true,
  }));
  registerInvokeHandler("sources_list", async () => [
    {
      id: ONBOARDED_SOURCE_ID,
      kind: "LocalGit",
      label: "work repos",
      config: { LocalGit: { scan_roots: ["/Users/me/code"] } },
      secret_ref: null,
      created_at: "2026-04-17T12:00:00Z",
      last_sync_at: null,
      last_health: { ok: true, checked_at: null, last_error: null },
    },
  ]);
  registerInvokeHandler("identities_list_for", async () => [
    {
      id: ONBOARDED_IDENTITY_ID,
      person_id: ONBOARDED_PERSON_ID,
      source_id: null,
      kind: "GitEmail",
      external_actor_id: "vedanth@example.com",
    },
  ]);
  registerInvokeHandler("sinks_list", async () => [
    {
      id: ONBOARDED_SINK_ID,
      kind: "MarkdownFile",
      label: "daily notes",
      config: {
        MarkdownFile: {
          config_version: 1,
          dest_dirs: ["/Users/me/notes"],
          frontmatter: false,
        },
      },
      created_at: "2026-04-17T12:00:00Z",
      last_write_at: null,
    },
  ]);
  // The source chip now surfaces the discovered-repo count, so the
  // fully-onboarded fixture needs a populated `local_repos_list`
  // response — otherwise the chip would stay on "…" or "0 repos"
  // and every rendered snapshot would include that unrealistic
  // state.
  registerInvokeHandler("local_repos_list", async () => [
    {
      path: "/Users/me/code/alpha",
      label: "alpha",
      is_private: false,
      discovered_at: "2026-04-17T12:00:00Z",
    },
    {
      path: "/Users/me/code/beta",
      label: "beta",
      is_private: false,
      discovered_at: "2026-04-17T12:00:00Z",
    },
  ]);
}
