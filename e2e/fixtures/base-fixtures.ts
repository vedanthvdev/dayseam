// Single entry point that every step definition imports from. We
// `mergeTests` the runtime fixture (page-scoped: Tauri mock install
// + diagnostics capture) with the page-object fixture (step-facing
// `pages` factory), then pass the merged `test` into `createBdd` so
// Given/When/Then/Before/After all receive the same extended
// fixture surface.
//
// Keeping this the only place that calls `createBdd(test)` means a
// new step file is always one import line away from the full
// fixture set, and adding a new fixture is one `mergeTests` entry
// here — never touched by the step files themselves.

import { mergeTests } from "@playwright/test";
import { createBdd } from "playwright-bdd";
import { test as PageFixture } from "./pages/page-fixture";
import { test as RuntimeFixtures } from "./runtime/runtime-fixtures";

export const test = mergeTests(RuntimeFixtures, PageFixture);
export const { Given, When, Then, Before, After } = createBdd(test);
