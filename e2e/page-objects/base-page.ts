// Shared base class for every E2E page object.
//
// A page object is deliberately thin: it owns the Playwright `Page`
// handle, exposes a small surface of intent-named methods, and never
// leaks selectors up to the step definitions. Keeping a common base
// keeps the `constructor(page)` boilerplate in one place and lets
// future cross-cutting behaviour (e.g. standardised waits, trace
// annotations) land without touching every page.

import type { Page } from "@playwright/test";

export abstract class BasePage {
  protected readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }
}
