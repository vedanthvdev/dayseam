// Playwright config for the Dayseam BDD-flavoured E2E suite.
//
// The test surface is Gherkin `.feature` files under `features/`;
// `playwright-bdd`'s `defineBddConfig` compiles them into runnable
// Playwright specs under `.features-gen/` (gitignored, regenerated
// on every run). That compiled dir is what Playwright actually
// picks up via `testDir`, so the standard `playwright test` command
// still works — `bddgen` in the `e2e`/`e2e:headed`/`e2e:ui` scripts
// just ensures the compilation happened first.
//
// We drive the built Vite bundle (produced by `@dayseam/desktop`)
// in a real Chromium with the Tauri IPC boundary mocked in-page via
// `fixtures/runtime/tauri-mock-init.ts`. That covers the full user
// click path (navigation, forms, IPC argument shape, progress
// streams, window events, save flow) without needing a real Tauri
// runtime. The Rust side has its own integration coverage — see
// `dayseam-orchestrator::multi_source_dedups_commitauthored` for the
// cross-source rollup and `sink-markdown-file`'s marker-block tests
// for the round-trip. A real `.app` bundle driver (tauri-driver +
// WebDriverIO on macOS) is tracked as a follow-up; WebKit's
// WebDriver story is thin enough on macOS 13+ that shipping the
// mocked-IPC variant now lets every PR run E2E on Linux cheaply
// while we wait for a reliable native driver.

import { defineConfig, devices } from "@playwright/test";
import { defineBddConfig } from "playwright-bdd";

// `defineBddConfig` returns the path Playwright should treat as its
// `testDir`. Keep the arguments here minimal and declarative: what
// are the features, where are the steps, which `test` do the steps
// attach to. `statefulPoms: true` tells playwright-bdd our page
// objects may hold state between steps — our `PageFactory` is
// instantiated once per scenario and hands references to `Page`
// through to each page object, so the flag is the honest choice.
// `steps` covers both the step-definition files and the
// `base-fixtures.ts` that `createBdd(test)` lives in, so
// `playwright-bdd` picks up the extended `test` surface without
// a separate `importTestFrom` entry (deprecated in v8+).
const testDir = defineBddConfig({
  features: "features/**/*.feature",
  steps: ["steps/**/*.ts", "fixtures/base-fixtures.ts"],
  statefulPoms: true,
});

// Per Task 5 invariant #1: the happy path has a three-minute wall-clock
// budget on CI. The per-test timeout enforces it so a future change that
// pushes the run past the budget shows up as a CI red, not as a
// slow-creep regression.
const THREE_MINUTES_MS = 3 * 60 * 1000;

export default defineConfig({
  testDir,
  timeout: THREE_MINUTES_MS,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  // Task 5 step 5.4: no retry-masking. A flaky test is a test that gets
  // rewritten before the PR opens, not one CI papers over.
  retries: 0,
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI
    ? [["list"], ["html", { outputFolder: "report", open: "never" }]]
    : [["list"], ["html", { outputFolder: "report", open: "on-failure" }]],
  use: {
    baseURL: "http://localhost:4173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    viewport: { width: 1280, height: 800 },
  },
  // We serve the production bundle (`vite preview`) out of
  // `apps/desktop/dist`, not the dev server, so the test exercises the
  // same artefact the Tauri bundler ships. `pnpm --filter ... exec`
  // runs the command with `apps/desktop` as cwd, which is where
  // `vite preview` looks for `dist/`. `reuseExistingServer` keeps
  // local iteration snappy; on CI we always boot a fresh one.
  webServer: {
    command:
      "pnpm --filter @dayseam/desktop exec vite preview --port=4173 --strictPort --host=127.0.0.1",
    url: "http://localhost:4173",
    reuseExistingServer: !process.env.CI,
    stdout: "pipe",
    stderr: "pipe",
    timeout: 120_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
