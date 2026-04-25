// In-process mock for the subset of `@tauri-apps/api` we use in tests.
// Tests import helpers from here to register invoke handlers, fire
// event-bus messages, and inspect channel writes without ever going
// near a real Tauri webview.

import { vi } from "vitest";

type InvokeHandler = (args: Record<string, unknown>) => unknown | Promise<unknown>;

const invokeHandlers = new Map<string, InvokeHandler>();

// Every in-flight invoke call is tracked so `waitForPendingInvokes()`
// can drain them before teardown. React hooks that fire IPC from
// `useEffect` chain setState calls off these promises; if the test
// ends before they settle, the setState lands on a still-mounted
// component *after* the test body — which shows up as a React
// "update was not wrapped in act(...)" warning and (under the
// TST-05 setup.ts guard) fails the next test. The global afterEach
// in `setup.ts` awaits this set inside `act(...)` to close them out
// deterministically.
const pendingInvokes = new Set<Promise<unknown>>();

export const mockInvoke = vi.fn(
  async (name: string, args: Record<string, unknown> = {}) => {
    const handler = invokeHandlers.get(name);
    if (!handler) {
      throw new Error(`No mock invoke handler registered for "${name}"`);
    }
    const promise = (async () => handler(args))();
    pendingInvokes.add(promise);
    try {
      return await promise;
    } finally {
      pendingInvokes.delete(promise);
    }
  },
);

export function registerInvokeHandler(name: string, handler: InvokeHandler): void {
  invokeHandlers.set(name, handler);
}

export function resetInvokeHandlers(): void {
  invokeHandlers.clear();
  mockInvoke.mockClear();
}

/** Await every in-flight `invoke` call. Safe to call repeatedly: it
 *  snapshots the current set, awaits each entry (swallowing
 *  rejection so one failing handler doesn't mask the others),
 *  then yields to the event loop so the consumer's continuation
 *  (which fires the trailing `setState` in `finally`) gets to run.
 *  A resolved invoke can trigger React state updates that fire
 *  *another* invoke synchronously (e.g. `sources_list` → render →
 *  `local_repos_list`), so we loop until the set is stable at
 *  zero across two consecutive macrotask ticks. Used by the
 *  global teardown flush in `setup.ts`. Bounded at 20 rounds so
 *  a runaway infinite-invoke loop in a test fails loudly instead
 *  of hanging CI. */
export async function waitForPendingInvokes(): Promise<void> {
  for (let round = 0; round < 20; round += 1) {
    if (pendingInvokes.size === 0) {
      // Yield once more so consumer continuations off the last
      // batch get a chance to schedule follow-up invokes; if none
      // do, we exit cleanly on the next iteration.
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      if (pendingInvokes.size === 0) return;
    }
    const snapshot = [...pendingInvokes];
    await Promise.allSettled(snapshot);
    // Macrotask hop: lets every consumer's `.then` / `finally`
    // continuation run, including the trailing `setState` calls
    // the TST-05 guard is watching for.
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
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

// `@tauri-apps/plugin-updater` / `@tauri-apps/plugin-process` are
// mocked in `setup.ts`; tests drive them via the queue and spy
// exported here. The updater mock resolves to whatever the head of
// `updateCheckQueue` returns (or `null` when empty, matching the
// real plugin's "no update available" contract). The process mock
// is a plain spy so tests can assert on `mockRelaunch` invocation
// counts without having to thread it through a handler registry.
//
// `MockUpdate` mimics enough of the real `Update` resource for the
// hook: a `version`, `currentVersion`, optional `body`, a
// `downloadAndInstall(onEvent)` that fires a scripted sequence of
// download events and resolves, and a `close()` that bumps the
// internal counter so tests can assert cleanup.

type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

export interface MockUpdateInit {
  version: string;
  currentVersion: string;
  body?: string;
  downloadEvents?: DownloadEvent[];
  downloadError?: Error;
}

export class MockUpdate {
  version: string;
  currentVersion: string;
  body?: string;
  rawJson: Record<string, unknown> = {};
  date?: string;
  available = true;
  rid = Math.floor(Math.random() * 1_000_000);
  closeCalls = 0;
  installCalls = 0;
  private events: DownloadEvent[];
  private downloadError?: Error;

  constructor(init: MockUpdateInit) {
    this.version = init.version;
    this.currentVersion = init.currentVersion;
    this.body = init.body;
    this.events = init.downloadEvents ?? [
      { event: "Started", data: { contentLength: 100 } },
      { event: "Progress", data: { chunkLength: 50 } },
      { event: "Progress", data: { chunkLength: 50 } },
      { event: "Finished" },
    ];
    this.downloadError = init.downloadError;
  }

  async downloadAndInstall(
    onEvent?: (event: DownloadEvent) => void,
  ): Promise<void> {
    this.installCalls += 1;
    for (const event of this.events) {
      onEvent?.(event);
    }
    if (this.downloadError) throw this.downloadError;
  }

  async download(): Promise<void> {
    throw new Error("MockUpdate.download should never be called by useUpdater");
  }

  async install(): Promise<void> {
    throw new Error("MockUpdate.install should never be called by useUpdater");
  }

  async close(): Promise<void> {
    this.closeCalls += 1;
  }
}

const updateCheckQueue: Array<MockUpdate | null | Error> = [];

export const mockUpdaterCheck = vi.fn(async (): Promise<MockUpdate | null> => {
  if (updateCheckQueue.length === 0) return null;
  const next = updateCheckQueue.shift()!;
  if (next instanceof Error) throw next;
  return next;
});

/** Script the next `check()` responses. Pass a `MockUpdate` for
 *  "update available", `null` for "up to date", or an `Error` for
 *  "check failed". */
export function queueUpdaterCheck(
  ...responses: Array<MockUpdate | null | Error>
): void {
  updateCheckQueue.push(...responses);
}

export function resetUpdaterPlugin(): void {
  updateCheckQueue.length = 0;
  mockUpdaterCheck.mockClear();
}

export const mockRelaunch = vi.fn(async () => {});

export function resetProcessPlugin(): void {
  mockRelaunch.mockClear();
}

/** One-call reset used by `beforeEach` in tests. */
export function resetTauriMocks(): void {
  resetInvokeHandlers();
  resetChannels();
  resetEventBus();
  resetDialogPlugin();
  resetUpdaterPlugin();
  resetProcessPlugin();
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
  // DAY-149: `PreferencesDialog` is always mounted on `App`, which
  // means its new `useSettings` hook fires `settings_get` on every
  // render. Defaulting the response to a fresh install's values
  // here — rather than forcing every App-level test to register it
  // manually — keeps the fixture honest to what the first-launch
  // user actually sees. Tests that care about a specific settings
  // shape (e.g. a user who already turned background mode off)
  // override these handlers in their own body.
  registerInvokeHandler("settings_get", async () => ({
    config_version: 2,
    theme: "system",
    verbose_logs: false,
    keep_running_when_window_closed: true,
  }));
  registerInvokeHandler("settings_update", async (args) => {
    const { patch } = args as { patch: Record<string, unknown> };
    return {
      config_version: 2,
      theme: "system",
      verbose_logs: false,
      keep_running_when_window_closed: true,
      ...patch,
    };
  });
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
