// Page object for the report generation flow.
//
// `clickGenerate` kicks the renderer-side orchestration: IPC
// `report_generate`, the progress `Channel`, and the `report:completed`
// window event. `expectDraftVisible` gates on the streaming preview
// flipping into its `completed` state, which is the single visible
// proof that the whole pipeline ran. A generous 30s timeout leaves
// headroom for CI cold starts without papering over a real stall.
//
// ## DAY-90 TST-v0.2-02 — count-aware assertions
//
// `expectDraftContains` is the "does the string appear anywhere in
// the draft" assertion. It's useful as a smoke check but it passes
// if a heading merely renders; a regression where the draft stops
// emitting any bullets (but the section title still renders) slips
// past. The `*bySection*` helpers below scope to a specific
// `data-section` id and assert on bullet **counts** and on
// per-section bullet text, so a drift between "mock said there are
// two bullets" and "UI rendered one" fails the assertion.
//
// The `data-section` + `data-bullet` hooks live on the renderer
// (see `SectionView` / `BulletRow` in `StreamingPreview.tsx`).

import { expect } from "@playwright/test";
import { BasePage } from "../base-page";
import { ReportLocators } from "./report-locators";

const DRAFT_VISIBLE_TIMEOUT_MS = 30_000;

export class ReportPage extends BasePage {
  async clickGenerate(): Promise<void> {
    await this.page.getByTestId(ReportLocators.GENERATE_BUTTON).click();
  }

  async expectDraftVisible(): Promise<void> {
    const draft = this.page.getByTestId(ReportLocators.STREAMING_PREVIEW_DRAFT);
    await expect(draft).toBeVisible({ timeout: DRAFT_VISIBLE_TIMEOUT_MS });
  }

  async expectDraftContains(text: string): Promise<void> {
    const draft = this.page.getByTestId(ReportLocators.STREAMING_PREVIEW_DRAFT);
    await expect(draft).toContainText(text);
  }

  /// Scope a Playwright locator to the `<section data-section="…">`
  /// block inside the draft. Private helper for the count-aware
  /// assertions below; callers should use those rather than
  /// re-rolling the selector string at every call site.
  private section(sectionId: string) {
    const draft = this.page.getByTestId(ReportLocators.STREAMING_PREVIEW_DRAFT);
    return draft.locator(`[data-section="${sectionId}"]`);
  }

  /// Assert that `sectionId` contains exactly `expected` bullets.
  /// Uses `toHaveCount` so the assertion auto-retries while the
  /// streaming preview is still filling in — a section that
  /// *temporarily* shows one bullet on the way to three won't
  /// flake, but a section that permanently renders the wrong count
  /// will fail.
  async expectSectionBulletCount(
    sectionId: string,
    expected: number,
  ): Promise<void> {
    const section = this.section(sectionId);
    await expect(section).toBeVisible({ timeout: DRAFT_VISIBLE_TIMEOUT_MS });
    await expect(section.locator("[data-bullet]")).toHaveCount(expected);
  }

  /// Assert that `sectionId` contains exactly one bullet whose
  /// visible text includes `text`. Count-aware variant of
  /// `expectDraftContains`: a string that shows up twice (once as
  /// a section title, once inside a bullet) or zero times (section
  /// rendered empty) both fail loudly.
  async expectSectionContainsBullet(
    sectionId: string,
    text: string,
  ): Promise<void> {
    const section = this.section(sectionId);
    await expect(section).toBeVisible({ timeout: DRAFT_VISIBLE_TIMEOUT_MS });
    await expect(
      section.locator("[data-bullet]").filter({ hasText: text }),
    ).toHaveCount(1);
  }
}
