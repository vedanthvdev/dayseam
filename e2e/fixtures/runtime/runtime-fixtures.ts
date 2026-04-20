// Runtime fixtures wire the browser `page` up with everything a step
// definition needs to trust when it talks to the app:
//
//   1. `page.addInitScript(dayseamTauriMockInit)` puts a working
//      `window.__TAURI_INTERNALS__` in place before any frontend JS
//      runs, so the app's `@tauri-apps/api/core::invoke` immediately
//      talks to our in-page mock instead of a missing backend.
//
//   2. Console and page-error listeners collect anything the app
//      logs during a scenario. The `diagnostics.assertClean()` step
//      at the end of every feature turns that list into a failure
//      with the actual message — so a regression surfaces as
//      "Uncaught TypeError: …" rather than a generic timeout.
//
//   3. `tauriMock.capturedSaveCalls()` reaches back into the mock's
//      state on `window.__DAYSEAM_E2E__` so a Then step can assert
//      the exact IPC payload the renderer sent, not just that some
//      UI strings appeared.
//
// Bundling all three into one fixture (rather than two fixtures that
// both override `page`) sidesteps `mergeTests`' last-override-wins
// behaviour on duplicate keys: there is exactly one `page` override,
// and it owns both the mock install and the diagnostic listeners.

import { expect } from "@playwright/test";
import { test as base } from "playwright-bdd";
import { CATALOGUE } from "./catalogue";
import { dayseamTauriMockInit } from "./tauri-mock-init";
import type { MockState } from "./types";

type CapturedSaveCall = MockState["captured"]["saveCalls"][number];

type DiagnosticsState = {
  consoleErrors: string[];
  pageErrors: Error[];
};

export interface TauriMockFixture {
  capturedSaveCalls(): Promise<CapturedSaveCall[]>;
}

export interface DiagnosticsFixture {
  assertClean(): void;
}

export interface RuntimeFixtures {
  tauriMock: TauriMockFixture;
  diagnostics: DiagnosticsFixture;
}

// The React DevTools install notice fires on every fresh React build
// and is not actionable; filtering it here keeps `assertClean` a
// precise regression signal rather than a noise channel.
const NOISE_FRAGMENTS: readonly string[] = ["Download the React DevTools"];

function isActionableConsoleError(msg: string): boolean {
  return !NOISE_FRAGMENTS.some((fragment) => msg.includes(fragment));
}

export const test = base.extend<
  RuntimeFixtures & { _diagnosticsState: DiagnosticsState }
>({
  // Playwright fixture factories must accept a destructured object
  // from the fixture runner even when nothing upstream is consumed;
  // the empty pattern is load-bearing, not a mistake.
  _diagnosticsState: [
    // eslint-disable-next-line no-empty-pattern
    async ({}, use) => {
      await use({ consoleErrors: [], pageErrors: [] });
    },
    { scope: "test" },
  ],

  page: async ({ page, _diagnosticsState }, use) => {
    // Thread `CATALOGUE` into the page as a JSON-serialised arg so
    // the mock and the Node-side step assertions read from the
    // same source of truth (see `./catalogue.ts`). Playwright
    // serialises the second argument with `JSON.stringify`, so the
    // catalogue is deliberately plain data — no methods, no
    // classes, no cycles.
    await page.addInitScript(dayseamTauriMockInit, CATALOGUE);

    page.on("console", (msg) => {
      if (msg.type() === "error") {
        _diagnosticsState.consoleErrors.push(msg.text());
      }
    });
    page.on("pageerror", (err) => {
      _diagnosticsState.pageErrors.push(err);
    });

    await use(page);
  },

  tauriMock: async ({ page }, use) => {
    await use({
      async capturedSaveCalls(): Promise<CapturedSaveCall[]> {
        return page.evaluate(() => {
          const state = (
            window as unknown as { __DAYSEAM_E2E__?: MockState }
          ).__DAYSEAM_E2E__;
          return state?.captured.saveCalls ?? [];
        });
      },
    });
  },

  diagnostics: async ({ _diagnosticsState }, use) => {
    await use({
      assertClean(): void {
        const actionable =
          _diagnosticsState.consoleErrors.filter(isActionableConsoleError);

        expect(
          actionable,
          `console.error output during scenario:\n${actionable.join("\n")}`,
        ).toEqual([]);
        expect(
          _diagnosticsState.pageErrors,
          `uncaught page errors during scenario:\n${_diagnosticsState.pageErrors
            .map((e) => e.stack ?? e.message)
            .join("\n---\n")}`,
        ).toEqual([]);
      },
    });
  },
});
