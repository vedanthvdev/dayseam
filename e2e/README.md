# `@dayseam/e2e`

Root-level workspace package that owns Dayseam's end-to-end suite.
Scenarios live in **Gherkin `.feature` files** so the catalogue of
what this layer actually covers reads in plain English; Playwright
drives them via [`playwright-bdd`][pbdd], in a real Chromium, against
the production Vite bundle built by `@dayseam/desktop`, with the
Tauri IPC boundary mocked in-page.

E2E lives at the repo root (alongside `apps/`, `crates/`,
`packages/`) rather than nested inside `apps/desktop/` so the folder
structure surfaces the test layer at a glance and so neither
`@playwright/test` nor `playwright-bdd` sit in the desktop app's dep
closure.

[pbdd]: https://vitalets.github.io/playwright-bdd/

## What this test is and isn't

**Is**: end-to-end coverage of the user-visible click path. Each
scenario drives the same compiled bundle the Tauri `.app` ships (via
`vite preview`) and exercises every frontend component, hook, router,
and IPC-wire shape in a real browser. The mock-IPC layer implements
`window.__TAURI_INTERNALS__` to the same contract `@tauri-apps/api`
expects, so a drift in command name, argument shape, channel usage,
or `report:completed` event payload surfaces as a test failure.

**Isn't**: a test of the Rust orchestrator, SQLite, or the on-disk
markdown sink. Those live in Rust integration tests
(`multi_source_dedups_commitauthored`, `sink-markdown-file`'s
marker-block round-trip, etc.). The Phase 3 plan originally called
for `tauri-driver` against the real `.app` bundle; on macOS 13+
WKWebView's WebDriver support is thin enough that shipping the
mocked-IPC variant now lets every PR run E2E cheaply on Linux while
we track a native-driver follow-up separately.

## Running locally

```bash
# from the repo root — build the bundle the test serves, then run
pnpm --filter @dayseam/desktop build
pnpm --filter @dayseam/e2e e2e            # headless

# or, from the e2e/ package directory
cd e2e
pnpm e2e                                  # headless
pnpm e2e:headed                           # watch the browser
pnpm e2e:ui                               # Playwright's watch UI

# regenerate the compiled specs without running them (handy while
# iterating on a .feature and checking what was produced)
pnpm bddgen

# first-time local setup: download the Playwright Chromium build
pnpm --filter @dayseam/e2e playwright:install
```

`pnpm e2e` runs `bddgen` first (compiling every `.feature` into a
Playwright spec under `.features-gen/`) and then `playwright test`.
The `webServer` config boots `vite preview` on `127.0.0.1:4173`
automatically via `pnpm --filter @dayseam/desktop exec`, so you
never have to start the preview server by hand. If you already have
a preview server on that port (locally it's reused so iteration
stays fast), Playwright picks it up instead of spawning a second
one.

### Running a single scenario

Every scenario is tagged; filter with Playwright's `--grep` against
the tag. Examples:

```bash
pnpm e2e --grep @smoke              # only the smoke scenarios
pnpm e2e --grep @save-ipc-contract  # only the save-IPC assertion
```

### Tag taxonomy

Tags are the `--grep` handle CI and authors use to slice the suite.
Pick from the existing families before inventing a new one — an
unruly tag vocabulary is the first thing that rots a BDD suite.

- `@<domain>` — the primary surface the scenario covers.
  Current families: `@happy-path`, `@atlassian`, and (soon)
  `@sources`, `@identities`, `@sinks`, `@reports`, `@onboarding`.
  A scenario has **exactly one** `@<domain>` tag at the feature
  level.
- `@smoke` — scenario is cheap and critical enough to gate every
  PR. The CI workflow runs the full suite today; the tag future-
  proofs a "pre-merge subset" split when the suite grows past the
  PR budget.
- `@<feature>-ipc-contract` — scenario asserts on a captured IPC
  payload (e.g. `@save-ipc-contract`). Signals that a change to the
  Rust-side command shape will fail this scenario loudly; reviewers
  should treat flake here as a real regression, not a retry target.
- `@connector:<kind>` — scenario only makes sense for a specific
  connector (`@connector:gitlab`, `@connector:local-git`,
  `@connector:atlassian`). Use on connector-specific scenarios so
  a CI job that lacks the connector's fixtures can `--grep` them
  out, and so `--grep @connector:atlassian` becomes the one-liner
  answer to "did my change break Jira + Confluence?".

New tags should be added to this list in the same PR that
introduces them.

## Layout

```
e2e/
  package.json                     # @dayseam/e2e — playwright + playwright-bdd, own lint/typecheck
  playwright.config.ts             # Chromium-only, 3-minute per-test budget, defineBddConfig
  tsconfig.json                    # Self-contained; doesn't reach into apps/desktop
  eslint.config.js                 # Minimal; no React/refresh rules leak in

  features/                        # ← the human-readable scenario catalogue
    happy-path/
      generate-and-save-report.feature
    atlassian/
      connect-and-report.feature   # DAY-83: Jira-only, Confluence-only, both

  steps/                           # Gherkin → TypeScript bindings
    meta-steps.ts                  # cross-cutting (e.g. console-error guard)
    ui-steps/
      app-shell/
        app-shell-steps.ts         # "the Dayseam desktop app is open on the main screen"
      atlassian/
        atlassian-steps.ts         # Add-Atlassian-source dialog + per-product bullets
      report/
        report-steps.ts            # Generate → streaming preview → draft text
      save/
        save-steps.ts              # Save dialog → receipt → captured IPC

  page-objects/                    # one class per surface the steps talk to
    base-page.ts                   # shared Page-handle carrier
    app-shell/
      app-shell-page.ts
      app-shell-locators.ts
    atlassian/
      atlassian-dialog-page.ts
      atlassian-dialog-locators.ts
    report/
      report-page.ts
      report-locators.ts
    save/
      save-dialog-page.ts
      save-dialog-locators.ts

  fixtures/
    base-fixtures.ts               # mergeTests + createBdd — the one file steps import from
    pages/
      page-fixture.ts              # injects `pages` (a PageFactory) into every step
      page-factory.ts
    runtime/
      catalogue.ts                 # single source of truth for fixture ids/paths/text
      runtime-fixtures.ts          # installs the Tauri mock, captures console/page errors,
                                   # exposes `tauriMock` + `diagnostics` to steps
      tauri-mock-init.ts           # injected into the page via addInitScript;
                                   # handlers clustered by domain, names type-checked
                                   # against `@dayseam/ipc-types::Commands`
      types.ts                     # shared types for the mock

  .features-gen/                   # ← compiled specs (git-ignored, regenerated each run)
  report/                          # Playwright HTML report (git-ignored)
  test-results/                    # Per-run traces/screenshots (git-ignored)
```

The layout is loosely modelled after Modulr's `customer-portal-v2`
Playwright suite (features / steps / page-objects / fixtures) but
flattened: our single workspace package doesn't need Angular-CLI's
`src/tests/` envelope. Naming conventions match theirs
(`<domain>-steps.ts`, `<domain>-page.ts`, `<domain>-page-locators.ts`,
`@tag` scenarios) so a reader familiar with one can navigate the
other immediately.

## Authoring a new scenario

1. **Write the feature first.** Add (or extend) a `.feature` file
   under `features/<domain>/`. Use `Background:` for preconditions
   every scenario in that file shares; tag scenarios with
   `@domain @scenario-name` so they're greppable.
2. **Bind the steps.** For each new Given/When/Then, add a step
   definition under `steps/ui-steps/<domain>/<domain>-steps.ts`
   that calls through to the matching page object. Step bodies
   should read as one-liners — any meaningful logic belongs in the
   page object or a fixture. A new domain gets its own folder so a
   contributor can tell at a glance which connectors/surfaces the
   suite covers.
3. **Extend the page object.** If the scenario touches a new screen,
   add a page object under `page-objects/<domain>/` with its
   `-locators.ts` sibling. Keep selectors out of the step code: steps
   call `pages.save.selectConfiguredSink()`, not
   `page.getByTestId('save-sink-…').check()`.
4. **Wire new fixtures sparingly.** If the scenario needs data that
   neither `pages`, `tauriMock`, nor `diagnostics` provide, extend
   `runtime-fixtures.ts` (for browser-side state) or add a sibling
   fixture file and merge it into `base-fixtures.ts`.
5. **Run it.** `pnpm e2e --grep @your-tag` three times locally; if
   it's not stable at three, the scenario is not done. Invariant
   5.4: no retry-masking.

## Refreshing fixtures

The fixture surface today is entirely in-memory. Every id, path, and
piece of human-visible draft text the mock ships lives in a single
`fixtures/runtime/catalogue.ts` constant, and `runtime-fixtures.ts`
threads it into the browser via
`page.addInitScript(dayseamTauriMockInit, CATALOGUE)`. The mock
(page context) and the step assertions (Node context) therefore
read from the same source of truth — drift would be a compile-time
error, not a flake. Rules:

1. **Single edit point.** Need to change the draft text or the
   save path a scenario expects? Edit `catalogue.ts`. The mock,
   the `selectConfiguredSink` page-object call, and the
   `report_save` IPC-contract assertion pick it up automatically.
2. **Plain data only.** The catalogue is JSON-serialised by
   Playwright; no methods, no classes, no cycles.
3. **Feature files stay English.** The `.feature` files still
   contain literal user-facing strings (a human reads them). They
   are asserted against catalogue-backed fixtures, so a divergence
   between "what the scenario claims" and "what the mock serves"
   fails loudly on the next run.

A future pass that needs richer data (e.g. asserting on a specific
`ActivityEvent` id, or shipping a per-scenario draft captured from
a real run) should:

1. Extend `catalogue.ts` with the new shape, or add a sibling
   `fixtures/runtime/drafts/<scenario>.json` and wire it into the
   handler for that command.
2. Load it into the mock via a per-scenario fixture override, or
   via a dedicated step that calls `page.evaluate(...)` to mutate
   `window.__DAYSEAM_E2E__.handlers[cmd]`.
3. Document the capture recipe here.

If the `ReportDraft` or any other IPC result shape changes in
`packages/ipc-types`, the draft literal inside
`tauri-mock-init.ts::defaultHandlers` must track it. The mock's
command-name surface is already pinned: `MOCK_HANDLED_COMMANDS` is
declared `satisfies readonly CommandName[]`, so renaming or
deleting a command in `@dayseam/ipc-types` turns into a
`pnpm typecheck` failure here before the suite ever runs.

## Failure modes the suite catches

- **Unknown IPC command** — mock throws immediately; the error
  surfaces as a Playwright test failure with the offending command
  name.
- **Command-name drift between Rust and the mock** — caught at
  compile time, not run time. `MOCK_HANDLED_COMMANDS` in
  `tauri-mock-init.ts` uses `satisfies readonly CommandName[]`,
  so renaming or removing a command in `@dayseam/ipc-types`
  fails `pnpm typecheck` inside `@dayseam/e2e` before a single
  scenario runs.
- **Channel/event wiring regression** — if `useReport.generate` stops
  driving `report:completed`, the streaming preview never flips to
  `completed` and the `the streaming preview shows the completed
  draft` step times out.
- **Save flow drift** — the `captured save IPC call targets the
  configured sink at …` step verifies the renderer actually sent
  `{ draftId, sinkId, destinations }` with the right values;
  breaking the wiring on either side fails here.
- **Console / page errors** — any `console.error` or uncaught
  exception during a scenario is captured by the `diagnostics`
  fixture and asserted at the end via `no console or page errors
  were captured during the run`. The React DevTools install notice
  is filtered (it's noise, not a regression signal).

## Running on CI

`.github/workflows/e2e.yml` boots a fresh Ubuntu runner, installs
Playwright's Chromium (cached against `pnpm-lock.yaml`), builds the
frontend bundle via `pnpm --filter @dayseam/desktop build`, and runs
`pnpm --filter @dayseam/e2e e2e`. On failure it uploads the
Playwright HTML report (`e2e/report/`) as an artifact with 14-day
retention so you can download the trace / screenshots / video
without re-running locally.

### Why there is no `test` script

The package deliberately exposes `e2e` (and `e2e:headed` / `e2e:ui`)
but not `test`. The repo-wide `frontend` CI job runs
`pnpm --recursive --if-present run test` to collect every package's
vitest-style unit/component suite, and that job does **not** build
the frontend bundle or install a Playwright browser. A `test` alias
here would force that job to either carry extra setup (making it
slow for everyone) or fail with "`dist` does not exist." The
dedicated `playwright` workflow (and the local
`pnpm --filter @dayseam/e2e e2e`) remain the only entry points to
the suite; keep it that way.
