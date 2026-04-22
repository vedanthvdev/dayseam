// Browser-context init script injected via `page.addInitScript()` so
// the frontend finds a working `window.__TAURI_INTERNALS__` before
// the app's own `@tauri-apps/api/core::invoke` ever fires.
//
// Why inline instead of `import` from `@tauri-apps/api/mocks`:
// `addInitScript` serialises a single function into the page
// context, so the script must be self-contained — no imports, no
// closures over outer-module state. We mirror the shape `mocks.js`
// exposes (`invoke`, `transformCallback`, `unregisterCallback`,
// `runCallback`, `callbacks`) plus the `plugin:event|*` surface that
// `shouldMockEvents` enables. The E2E state (commands, events,
// captured writes) is kept on `window.__DAYSEAM_E2E__` so the test
// body can reach back in via `page.evaluate(...)` to drive scripted
// flows (fire a progress event, assert a captured Save payload)
// without another round-trip.
//
// The function takes a `Catalogue` argument because fixture ids and
// paths must line up across the mock and the step assertions;
// `runtime-fixtures.ts` passes `CATALOGUE` to `addInitScript` so
// Playwright serialises it as JSON into the page.

import type { CommandName } from "@dayseam/ipc-types";
import type { Catalogue } from "./catalogue";
import type { MockHandlers, MockInvokeHandler, MockState } from "./types";

// Declared surface of the mock: every IPC command the happy path
// (and any future E2E scenario) exercises. `satisfies readonly
// CommandName[]` makes TypeScript fail the `typecheck` script if a
// mock entry isn't a real production/dev command — so a renamed or
// deleted Rust command surfaces as a compile error in this file,
// not as a silently-mocked ghost command. Adding a new mocked
// command means adding it here *and* writing its handler inside
// `defaultHandlers` below (the handler map's return type is keyed
// by this tuple, so the handler is forced to exist).
export const MOCK_HANDLED_COMMANDS = [
  "persons_get_self",
  "sources_list",
  "identities_list_for",
  "sinks_list",
  "local_repos_list",
  "logs_tail",
  "report_generate",
  "report_get",
  "report_save",
  "activity_events_get",
  "shell_open",
  // DAY-83: the Atlassian happy-path scenarios drive
  // `AddAtlassianSourceDialog` → validate → persist through the
  // real IPC surface. Adding both commands here (with
  // `satisfies readonly CommandName[]`) turns a rename on the
  // Rust side into a compile-time failure at the mock boundary.
  "atlassian_validate_credentials",
  "atlassian_sources_add",
  // DAY-87: Atlassian reconnect flow. The scenarios drive
  // `SourceErrorCard` → `AddAtlassianSourceDialog` (reconnect mode)
  // → `atlassian_sources_reconnect` → `sources_healthcheck`
  // through the real IPC surface. Listing the command here turns
  // a rename on the Rust side into a compile-time failure at the
  // mock boundary, same as the add-flow commands above.
  "atlassian_sources_reconnect",
  // DAY-99: the GitHub add-source scenarios drive
  // `AddGithubSourceDialog` → validate → persist through the real
  // IPC surface. Listing the commands here turns a rename on the
  // Rust side into a compile-time failure at the mock boundary,
  // same as the Atlassian commands above.
  "github_validate_credentials",
  "github_sources_add",
  "sources_healthcheck",
] as const satisfies readonly CommandName[];

export type MockedCommand = (typeof MOCK_HANDLED_COMMANDS)[number];

export function dayseamTauriMockInit(catalogue: Catalogue): void {
  // `addInitScript` serialises this function and re-evaluates it
  // inside the page; running the body inside an IIFE mirrors how
  // Tauri's own `mocks.js` ships, and keeps `window` the single
  // extension point.
  const globalWindow = window as unknown as {
    __TAURI_INTERNALS__?: Record<string, unknown>;
    __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    __DAYSEAM_E2E__?: MockState;
  };

  globalWindow.__TAURI_INTERNALS__ = globalWindow.__TAURI_INTERNALS__ ?? {};
  globalWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ =
    globalWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ ?? {};

  // ── Event-plugin surface ────────────────────────────────────────
  // Tauri's `listen(name, cb)` routes through `plugin:event|listen`,
  // which registers `cb` against `name` and returns an opaque id.
  // `emit(name, payload)` routes through `plugin:event|emit` and
  // fans the payload out to every registered callback. We mirror
  // that contract here; `shouldMockEvents: true` upstream is the
  // equivalent switch.
  const listeners = new Map<string, Array<number>>();

  const callbacks = new Map<number, (data: unknown) => void>();
  let nextCallbackId = 1;

  function registerCallback(
    callback: (data: unknown) => void,
    once = false,
  ): number {
    const id = nextCallbackId++;
    callbacks.set(id, (data) => {
      if (once) callbacks.delete(id);
      callback(data);
    });
    return id;
  }

  function unregisterCallback(id: number): void {
    callbacks.delete(id);
  }

  function runCallback(id: number, data: unknown): void {
    const cb = callbacks.get(id);
    if (cb) cb(data);
  }

  function handleListen(args: { event: string; handler: number }): number {
    const bucket = listeners.get(args.event) ?? [];
    bucket.push(args.handler);
    listeners.set(args.event, bucket);
    return args.handler;
  }

  function handleEmit(args: { event: string; payload: unknown }): null {
    const bucket = listeners.get(args.event) ?? [];
    for (const id of bucket) {
      runCallback(id, { event: args.event, payload: args.payload, id });
    }
    return null;
  }

  function handleUnlisten(args: { event: string; id: number }): void {
    const bucket = listeners.get(args.event);
    if (!bucket) return;
    const idx = bucket.indexOf(args.id);
    if (idx !== -1) bucket.splice(idx, 1);
  }

  // ── Mock state (observable from the test body) ─────────────────
  // We capture every invoke for later assertions (a drift between
  // frontend IPC calls and the mocked surface shows up as an
  // unhandled command, not a silently-green test) and keep the
  // structured handler map so a future scenario can override
  // per-command behaviour at runtime without editing this file.
  const state: MockState = {
    invocations: [],
    captured: {
      saveCalls: [],
      atlassianAddCalls: [],
      atlassianReconnectCalls: [],
      githubAddCalls: [],
    },
    handlers: defaultHandlers(),
  };

  async function invoke(
    cmd: string,
    args: Record<string, unknown> | undefined,
    _options?: unknown,
  ): Promise<unknown> {
    const payload = args ?? {};
    state.invocations.push({ cmd, args: sanitiseArgsForCapture(payload) });

    // Plugin event surface: listen / emit / unlisten route through
    // the in-page map above. No other plugin routes are used by the
    // happy path; add them here if a future scenario needs them.
    if (cmd === "plugin:event|listen") {
      return handleListen(payload as { event: string; handler: number });
    }
    if (cmd === "plugin:event|emit") {
      return handleEmit(payload as { event: string; payload: unknown });
    }
    if (cmd === "plugin:event|unlisten") {
      handleUnlisten(payload as { event: string; id: number });
      return null;
    }

    const handler = state.handlers[cmd];
    if (!handler) {
      throw new Error(`[dayseam-e2e] no mock handler for IPC command "${cmd}"`);
    }
    return handler(payload, {
      emit(event, eventPayload) {
        handleEmit({ event, payload: eventPayload });
      },
    });
  }

  // When the frontend passes a `Channel<T>` in `args`, `core.js`
  // serialises it to `__CHANNEL__:<id>` via `toJSON`. Keeping a
  // JSON-safe snapshot in `invocations` lets the test assert call
  // shape without carrying live handler references around.
  function sanitiseArgsForCapture(
    raw: Record<string, unknown>,
  ): Record<string, unknown> {
    return JSON.parse(
      JSON.stringify(raw, (_key, value) => {
        if (value && typeof value === "object" && "toJSON" in value) {
          return (value as { toJSON: () => unknown }).toJSON();
        }
        return value;
      }),
    ) as Record<string, unknown>;
  }

  // Pull a `Channel<T>` handler id out of a serialised argument.
  // The frontend passes the Channel instance itself, so at mock
  // time `args.progress` is still the live object and we can fire
  // `args.progress.onmessage(event)` directly — but we go through
  // `runCallback` to preserve the ordered-delivery semantics the
  // real runtime enforces (the `Channel` class gates on a
  // monotonically increasing `index`).
  function channelEmit(channel: unknown, index: number, message: unknown): void {
    const ch = channel as
      | { id?: number; onmessage?: (msg: unknown) => void }
      | undefined;
    if (!ch) return;
    if (typeof ch.id === "number") {
      runCallback(ch.id, { index, message });
      return;
    }
    ch.onmessage?.(message);
  }

  // ── Default command handlers ────────────────────────────────────
  // Grouped by domain so a new connector or flow can find the right
  // cluster quickly. The return type is keyed by
  // `MOCK_HANDLED_COMMANDS`, so TypeScript flags any handler the
  // mock claims to cover but hasn't actually implemented (and any
  // extra handlers that were never declared in the surface list).
  function defaultHandlers(): Record<MockedCommand, MockInvokeHandler> &
    MockHandlers {
    const nowIso = new Date().toISOString();

    // ── Domain: sources ──────────────────────────────────────────
    // Closure-local mutable list: `sources_list` reads from it,
    // `atlassian_sources_add` appends to it, `report_generate` /
    // `report_get` derive the bullet set from its current state.
    // Every scenario starts with the seeded LocalGit row so the
    // onboarding-complete gate stays satisfied; Atlassian rows are
    // added dynamically by the DAY-83 scenarios via the real
    // `AddAtlassianSourceDialog` click-path.
    const sources: Array<Record<string, unknown>> = [
      {
        id: catalogue.ids.source,
        kind: "LocalGit",
        label: catalogue.sources.label,
        config: {
          LocalGit: { scan_roots: [...catalogue.sources.scanRoots] },
        },
        secret_ref: null,
        created_at: nowIso,
        last_sync_at: null,
        last_health: { ok: true, checked_at: null, last_error: null },
      },
    ];

    // ── Domain: reporting ────────────────────────────────────────
    // `buildDraft()` is called on every `report_get` so the bullet
    // set reflects which sources are currently connected — a
    // scenario that adds a Jira row through the dialog sees the
    // Jira bullet; a scenario that adds Confluence sees the
    // Confluence bullet; Journey A sees both. The LocalGit baseline
    // bullets are always emitted so the happy-path scenario's
    // assertions keep working unchanged.
    function buildDraft(): Record<string, unknown> {
      const bullets: Array<{ id: string; text: string }> = [];
      catalogue.draft.completedBullets.forEach((text, idx) => {
        bullets.push({ id: `completed.${idx}`, text });
      });
      const hasJira = sources.some((s) => s.kind === "Jira");
      const hasConfluence = sources.some((s) => s.kind === "Confluence");
      const hasGithub = sources.some((s) => s.kind === "GitHub");
      if (hasJira) {
        bullets.push({
          id: "completed.atlassian.jira",
          text: catalogue.draft.atlassianJiraBullet,
        });
      }
      if (hasConfluence) {
        bullets.push({
          id: "completed.atlassian.confluence",
          text: catalogue.draft.atlassianConfluenceBullet,
        });
      }
      // DAY-100 — the GitHub happy-path scenario drives the real
      // `AddGithubSourceDialog` → validate → persist path, so once
      // the scenario clicks "Add source" the new row lands in
      // `sources` and the next `report_get` call surfaces a
      // deterministic PR bullet under `## Completed`. Kept
      // symmetrical with the Atlassian arms above so a regression
      // in the bullet-emission ordering fails here too.
      if (hasGithub) {
        bullets.push({
          id: "completed.github.pull_request",
          text: catalogue.draft.githubPullRequestBullet,
        });
      }

      const perSourceState: Record<string, unknown> = {};
      for (const src of sources) {
        perSourceState[String(src.id)] = {
          status: "Completed",
          started_at: nowIso,
          finished_at: nowIso,
          fetched_count: 2,
          error: null,
        };
      }

      return {
        id: catalogue.ids.draft,
        date: new Date().toISOString().slice(0, 10),
        template_id: catalogue.draft.templateId,
        template_version: catalogue.draft.templateVersion,
        sections: [
          {
            id: "completed",
            title: "Completed",
            bullets,
          },
        ],
        evidence: [],
        per_source_state: perSourceState,
        verbose_mode: false,
        generated_at: nowIso,
      };
    }

    const handlers: Record<MockedCommand, MockInvokeHandler> = {
      // ── Domain: people ────────────────────────────────────────
      persons_get_self: async () => ({
        id: catalogue.ids.person,
        display_name: catalogue.persons.selfDisplayName,
        is_self: true,
      }),

      // ── Domain: sources ───────────────────────────────────────
      sources_list: async () => sources.map((s) => ({ ...s })),
      local_repos_list: async () =>
        catalogue.sources.repos.map((repo) => ({
          path: repo.path,
          label: repo.label,
          is_private: false,
          discovered_at: nowIso,
        })),

      // ── Domain: identities ────────────────────────────────────
      identities_list_for: async () => [
        {
          id: catalogue.ids.identity,
          person_id: catalogue.ids.person,
          source_id: null,
          kind: "GitEmail",
          external_actor_id: catalogue.identities.gitEmail,
        },
      ],

      // ── Domain: sinks ─────────────────────────────────────────
      sinks_list: async () => [
        {
          id: catalogue.ids.sink,
          kind: "MarkdownFile",
          label: catalogue.sinks.label,
          config: {
            MarkdownFile: {
              config_version: 1,
              dest_dirs: [...catalogue.sinks.destDirs],
              frontmatter: false,
            },
          },
          created_at: nowIso,
          last_write_at: null,
        },
      ],

      // ── Domain: logs & activity ──────────────────────────────
      logs_tail: async () => [],
      activity_events_get: async () => [],

      // ── Domain: reports ───────────────────────────────────────
      report_generate: async (args, ctx) => {
        // Fire the progress stream + the `report:completed` window
        // event asynchronously so the React state machine
        // transitions from `starting` -> `running` -> `completed`
        // the same way it does against the real Rust side. We use
        // a short microtask chain rather than real wall-clock
        // delays so the happy path stays well inside the
        // three-minute budget even when CI is under load.
        queueMicrotask(() => {
          channelEmit(args.progress, 0, {
            run_id: catalogue.ids.run,
            source_id: null,
            phase: { status: "starting", message: "Starting run" },
            emitted_at: new Date().toISOString(),
          });
          channelEmit(args.progress, 1, {
            run_id: catalogue.ids.run,
            source_id: catalogue.ids.source,
            phase: {
              status: "in_progress",
              completed: 2,
              total: 2,
              message: "Walking local repos",
            },
            emitted_at: new Date().toISOString(),
          });
          channelEmit(args.progress, 2, {
            run_id: catalogue.ids.run,
            source_id: null,
            phase: { status: "completed", message: "Run finished" },
            emitted_at: new Date().toISOString(),
          });
          ctx.emit("report:completed", {
            run_id: catalogue.ids.run,
            status: "Completed",
            draft_id: catalogue.ids.draft,
            cancel_reason: null,
          });
        });
        return catalogue.ids.run;
      },
      report_get: async () => buildDraft(),
      report_save: async (args) => {
        // Capture the Save payload so the test body can assert the
        // marker-block contract end-to-end without a second IPC
        // round-trip.
        const destinations = [...catalogue.sinks.writtenDestinations];
        state.captured.saveCalls.push({
          draftId: String(args.draftId),
          sinkId: String(args.sinkId),
          destinations,
        });
        return [
          {
            run_id: catalogue.ids.run,
            sink_kind: "MarkdownFile",
            destinations_written: destinations,
            external_refs: [],
            bytes_written: 512,
            written_at: new Date().toISOString(),
          },
        ];
      },

      // ── Domain: shell ─────────────────────────────────────────
      shell_open: async () => null,

      // ── Domain: Atlassian add-source ──────────────────────────
      // DAY-83. Mirrors the Rust-side contract documented in
      // `apps/desktop/src-tauri/src/ipc/atlassian.rs`:
      //
      //   * `atlassian_validate_credentials` returns the
      //     `AtlassianAccountInfo` triple the dialog pins to the
      //     "Connected as …" ribbon and, on submit, stamps onto the
      //     fresh `SourceIdentity` via `atlassian_sources_add`.
      //   * `atlassian_sources_add` returns the freshly-inserted
      //     source rows (one for Journey B / C, two for Journey A)
      //     and appends them to the closure-local `sources` list
      //     so the next `sources_list` IPC reflects the connect —
      //     the same ordering the real Rust side provides.
      //
      // Every supplied argument is stashed on
      // `state.captured.atlassianAddCalls` so an
      // `@atlassian-add-ipc-contract` scenario can assert the
      // exact payload shape without a second round-trip.
      atlassian_validate_credentials: async (args) => {
        const emailArg = typeof args.email === "string" ? args.email : "";
        return {
          account_id: catalogue.atlassian.accountId,
          display_name: catalogue.atlassian.displayName,
          email: emailArg || catalogue.atlassian.email,
          cloud_id: catalogue.atlassian.cloudId,
        };
      },
      atlassian_sources_add: async (args) => {
        const workspaceUrl = String(args.workspaceUrl ?? "");
        const emailArg = String(args.email ?? "");
        const apiTokenRaw = args.apiToken;
        const apiToken =
          typeof apiTokenRaw === "string" ? apiTokenRaw : null;
        const accountId = String(args.accountId ?? "");
        const enableJira = Boolean(args.enableJira);
        const enableConfluence = Boolean(args.enableConfluence);
        const reuseSecretRefRaw = args.reuseSecretRef as
          | { keychain_service?: unknown; keychain_account?: unknown }
          | null
          | undefined;
        const reuseSecretRef =
          reuseSecretRefRaw && typeof reuseSecretRefRaw === "object"
            ? {
                keychain_service: String(reuseSecretRefRaw.keychain_service),
                keychain_account: String(reuseSecretRefRaw.keychain_account),
              }
            : null;

        state.captured.atlassianAddCalls.push({
          workspaceUrl,
          email: emailArg,
          apiToken,
          accountId,
          enableJira,
          enableConfluence,
          reuseSecretRef,
        });

        // Mirrors the Rust-side keychain contract: Journey A shares
        // a single `secret_ref` across both rows; Journey C mode 1
        // clones an existing one. We don't simulate Journey C mode
        // 2's separate secret_ref here — no DAY-83 scenario
        // exercises it, and the existing `AddAtlassianSourceDialog`
        // unit tests cover the branch.
        const sharedSecretRef = reuseSecretRef ?? {
          keychain_service: catalogue.atlassian.sharedSecretRef.keychain_service,
          keychain_account: catalogue.atlassian.sharedSecretRef.keychain_account,
        };

        const created: Array<Record<string, unknown>> = [];
        if (enableJira) {
          const row = {
            id: catalogue.ids.atlassianJiraSource,
            kind: "Jira",
            label: "Jira",
            config: {
              Jira: {
                workspace_url: workspaceUrl,
                email: emailArg,
              },
            },
            secret_ref: { ...sharedSecretRef },
            created_at: new Date().toISOString(),
            last_sync_at: null,
            last_health: { ok: true, checked_at: null, last_error: null },
          };
          sources.push(row);
          created.push(row);
        }
        if (enableConfluence) {
          const row = {
            id: catalogue.ids.atlassianConfluenceSource,
            kind: "Confluence",
            label: "Confluence",
            config: {
              Confluence: { workspace_url: workspaceUrl, email: emailArg },
            },
            secret_ref: { ...sharedSecretRef },
            created_at: new Date().toISOString(),
            last_sync_at: null,
            last_health: { ok: true, checked_at: null, last_error: null },
          };
          sources.push(row);
          created.push(row);
        }
        // `accountId` is retained on the captured payload so a
        // future test can assert we stamped the validated identity
        // onto the new row. The mock itself doesn't track
        // `SourceIdentity` rows — the Rust side owns that table.
        void accountId;
        void apiToken;
        return created;
      },

      // ── Domain: Atlassian reconnect (DAY-87) ──────────────────
      // Token-only reconnect. The Rust side validates the token
      // server-side via `/rest/api/3/myself`, enforces the bound
      // account id, rotates the keychain slot, and returns every
      // source id that shared the rotated `SecretRef`. The mock
      // mirrors that shape: we flip `last_health.ok` back to true
      // on each affected row and return the ids so the frontend's
      // follow-up `sources_healthcheck` lands on a freshly-green
      // chip. Journey-A shared-PAT scenarios get both ids back.
      atlassian_sources_reconnect: async (args) => {
        const sourceId = String(args.sourceId ?? "");
        const apiToken = String(args.apiToken ?? "");
        state.captured.atlassianReconnectCalls.push({ sourceId, apiToken });

        const target = sources.find((s) => s.id === sourceId);
        if (!target) {
          throw new Error(
            `[dayseam-e2e] atlassian_sources_reconnect: unknown source ${sourceId}`,
          );
        }
        const targetSecret = target.secret_ref as
          | { keychain_service: string; keychain_account: string }
          | null;
        const affected: string[] = [];
        for (const s of sources) {
          const sr = s.secret_ref as
            | { keychain_service: string; keychain_account: string }
            | null;
          const sameSlot =
            sr != null &&
            targetSecret != null &&
            sr.keychain_service === targetSecret.keychain_service &&
            sr.keychain_account === targetSecret.keychain_account;
          if (s.id === sourceId || sameSlot) {
            s.last_health = {
              ok: true,
              checked_at: new Date().toISOString(),
              last_error: null,
            };
            affected.push(String(s.id));
          }
        }
        return affected;
      },

      // ── Domain: GitHub add-source (DAY-99) ─────────────────────
      // Mirrors the Rust-side contract documented in
      // `apps/desktop/src-tauri/src/ipc/github.rs`:
      //
      //   * `github_validate_credentials` returns the
      //     `GithubValidationResult` triple the dialog pins to the
      //     "Connected as …" ribbon and, on submit, stamps onto the
      //     fresh `SourceIdentity` via `github_sources_add`.
      //   * `github_sources_add` returns the freshly-inserted source
      //     row and appends it to the closure-local `sources` list
      //     so the next `sources_list` IPC reflects the connect.
      //
      // Every supplied argument is stashed on
      // `state.captured.githubAddCalls` so a future
      // `@github-add-ipc-contract` scenario can assert the exact
      // payload shape without a second round-trip.
      github_validate_credentials: async () => ({
        user_id: catalogue.github.userId,
        login: catalogue.github.login,
        name: catalogue.github.name,
      }),
      github_sources_add: async (args) => {
        const apiBaseUrl = String(args.apiBaseUrl ?? "");
        const label = String(args.label ?? "");
        const pat =
          typeof args.pat === "string" ? args.pat : "";
        const userId =
          typeof args.userId === "number"
            ? args.userId
            : Number(args.userId ?? 0);

        state.captured.githubAddCalls.push({
          apiBaseUrl,
          label,
          pat,
          userId,
        });

        const row = {
          id: catalogue.ids.githubSource,
          kind: "GitHub",
          label: label || catalogue.github.label,
          config: {
            GitHub: { api_base_url: apiBaseUrl },
          },
          secret_ref: {
            keychain_service: catalogue.github.secretRef.keychain_service,
            keychain_account: catalogue.github.secretRef.keychain_account,
          },
          created_at: new Date().toISOString(),
          last_sync_at: null,
          last_health: { ok: true, checked_at: null, last_error: null },
        };
        sources.push(row);
        return row;
      },

      // Healthcheck is a simple "flip the row to ok, return the
      // current health snapshot" shim — the real Rust side runs a
      // connector-specific probe but the reconnect scenarios only
      // need the "chip turns green again" affordance.
      sources_healthcheck: async (args) => {
        const id = String(args.id ?? "");
        const target = sources.find((s) => s.id === id);
        if (!target) {
          throw new Error(
            `[dayseam-e2e] sources_healthcheck: unknown source ${id}`,
          );
        }
        target.last_health = {
          ok: true,
          checked_at: new Date().toISOString(),
          last_error: null,
        };
        return target.last_health;
      },
    };

    return handlers;
  }

  // Install the shims `core.js` reads:
  const tauri = globalWindow.__TAURI_INTERNALS__!;
  tauri.invoke = invoke;
  tauri.transformCallback = registerCallback;
  tauri.unregisterCallback = unregisterCallback;
  tauri.runCallback = runCallback;
  tauri.callbacks = callbacks;
  tauri.metadata = {
    currentWindow: { label: "main" },
    currentWebview: { windowLabel: "main", label: "main" },
  };

  // Matches `clearMocks()` semantics: the event-plugin surface needs
  // an `unregisterListener` so Tauri's `UnlistenFn` actually tears
  // down on component unmount.
  globalWindow.__TAURI_EVENT_PLUGIN_INTERNALS__!.unregisterListener = (
    _event: string,
    id: number,
  ) => unregisterCallback(id);

  globalWindow.__DAYSEAM_E2E__ = state;
}
