// Playwright fixture that injects a `PageFactory` into every step
// definition. Steps receive `{ pages }` and never construct page
// objects themselves, so the factory is the single place the Page
// handle flows through.

import { test as base } from "playwright-bdd";
import { PageFactory } from "./page-factory";

export interface PageFixture {
  pages: PageFactory;
}

export const test = base.extend<PageFixture>({
  pages: async ({ page }, use) => {
    await use(new PageFactory(page));
  },
});
