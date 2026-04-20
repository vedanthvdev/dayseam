// Page object for the Dayseam main application shell.
//
// Represents "the user has finished onboarding and is looking at the
// generate/save surface". Landing here (rather than the first-run
// empty state) is the gate every scenario starts from; if the
// onboarding fixture regresses, `openMainScreen` fails loudly instead
// of the later steps timing out against the wrong screen.

import { expect } from "@playwright/test";
import { BasePage } from "../base-page";
import { AppShellLocators } from "./app-shell-locators";

export class AppShellPage extends BasePage {
  async openMainScreen(): Promise<void> {
    await this.page.goto("/");
    const generate = this.page.getByTestId(AppShellLocators.GENERATE_BUTTON);
    await expect(generate).toBeVisible();
    await expect(generate).toBeEnabled();
  }
}
