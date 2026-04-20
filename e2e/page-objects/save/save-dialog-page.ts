// Page object for the Save Report dialog.
//
// The fixture ships exactly one sink; `selectConfiguredSink` picks it
// by id rather than "the only radio" so that adding a second sink in
// the future surfaces as a test-intent change (pick which sink?) and
// not a silent behaviour swap.

import { expect } from "@playwright/test";
import { CATALOGUE } from "../../fixtures/runtime/catalogue";
import { BasePage } from "../base-page";
import { SaveDialogLocators } from "./save-dialog-locators";

export class SaveDialogPage extends BasePage {
  async openDialog(): Promise<void> {
    await this.page
      .getByRole("button", { name: SaveDialogLocators.OPEN_SAVE_BUTTON_NAME })
      .click();
    await expect(this.page.getByTestId(SaveDialogLocators.DIALOG)).toBeVisible();
  }

  async selectConfiguredSink(): Promise<void> {
    await this.page
      .getByTestId(`${SaveDialogLocators.SINK_RADIO_PREFIX}${CATALOGUE.ids.sink}`)
      .check();
  }

  async confirm(): Promise<void> {
    await this.page
      .getByTestId(SaveDialogLocators.DIALOG)
      .getByRole("button", { name: SaveDialogLocators.CONFIRM_SAVE_BUTTON_NAME })
      .click();
  }

  async expectReceiptContains(path: string): Promise<void> {
    const receipts = this.page.getByTestId(SaveDialogLocators.RECEIPTS);
    await expect(receipts).toBeVisible();
    await expect(receipts).toContainText(path);
  }
}
