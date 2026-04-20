// Page object for the report generation flow.
//
// `clickGenerate` kicks the renderer-side orchestration: IPC
// `report_generate`, the progress `Channel`, and the `report:completed`
// window event. `expectDraftVisible` gates on the streaming preview
// flipping into its `completed` state, which is the single visible
// proof that the whole pipeline ran. A generous 30s timeout leaves
// headroom for CI cold starts without papering over a real stall.

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
}
