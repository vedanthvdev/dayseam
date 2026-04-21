// Page object for the Add-Atlassian-source flow.
//
// The entry point lives on the main-screen sidebar ("Add source" →
// "Add Atlassian source"); the dialog itself is the React component
// `AddAtlassianSourceDialog`. The DAY-83 scenarios drive this
// surface end-to-end: open the dialog, pick the product(s), paste
// the workspace URL / email / API token from the fixture catalogue,
// run the `atlassian_validate_credentials` probe, then submit —
// which issues `atlassian_sources_add` and returns the fresh row(s)
// that `sources_list` subsequently reflects.
//
// The page object deliberately exposes each distinct user intent
// (`selectOnlyJira`, `selectOnlyConfluence`, `selectBothProducts`,
// etc.) as its own method so the feature file reads as a user
// decision tree, not as a checkbox-toggling walkthrough.

import { expect } from "@playwright/test";
import { CATALOGUE } from "../../fixtures/runtime/catalogue";
import { BasePage } from "../base-page";
import { AtlassianDialogLocators } from "./atlassian-dialog-locators";

// Generous budget for the validate-button round-trip: the mock
// resolves synchronously, but the React state machine flips through
// `checking` → `ok` and renders the confirmation ribbon on the next
// microtask. Matching the `DRAFT_VISIBLE_TIMEOUT_MS` upper bound
// from `report-page.ts` keeps a cold-start CI runner's noise out of
// the signal.
const VALIDATION_VISIBLE_TIMEOUT_MS = 30_000;

export class AtlassianDialogPage extends BasePage {
  async openFromSidebar(): Promise<void> {
    await this.page
      .getByTestId(AtlassianDialogLocators.SIDEBAR_ADD_MENU_TRIGGER)
      .click();
    await this.page
      .getByTestId(AtlassianDialogLocators.SIDEBAR_ADD_MENU_ATLASSIAN)
      .click();
    await expect(
      this.page.getByTestId(AtlassianDialogLocators.DIALOG),
    ).toBeVisible();
  }

  /**
   * Leave Jira ticked, untick Confluence. Journey B — single product.
   */
  async selectOnlyJira(): Promise<void> {
    await this.setJira(true);
    await this.setConfluence(false);
  }

  /**
   * Untick Jira, leave Confluence ticked. Journey B — single product.
   */
  async selectOnlyConfluence(): Promise<void> {
    await this.setJira(false);
    await this.setConfluence(true);
  }

  /**
   * Tick both products. Journey A — shared PAT, one keychain row,
   * two `sources` rows.
   */
  async selectBothProducts(): Promise<void> {
    await this.setJira(true);
    await this.setConfluence(true);
  }

  /**
   * Fill the three credential fields from the Atlassian fixture.
   * The workspace URL is the bare slug so the scenario exercises
   * the `normaliseWorkspaceUrl` expansion path; the normalised
   * origin surfaces as the `add-atlassian-url-normalised` hint
   * which a Then step can assert when it wants to cover the
   * normalisation contract explicitly.
   */
  async fillCredentialsFromFixture(): Promise<void> {
    const workspaceField = this.page.getByTestId(
      AtlassianDialogLocators.WORKSPACE_URL,
    );
    await workspaceField.fill(CATALOGUE.atlassian.workspaceSlug);
    await this.page
      .getByTestId(AtlassianDialogLocators.EMAIL)
      .fill(CATALOGUE.atlassian.email);
    await this.page
      .getByTestId(AtlassianDialogLocators.API_TOKEN)
      .fill(CATALOGUE.atlassian.apiToken);
  }

  /**
   * Click Validate and wait for the `✓ Connected as …` ribbon —
   * the gate the dialog enforces before enabling the `Add source`
   * button. Failing validation surfaces as a
   * `add-atlassian-validation-error` node; we let the Playwright
   * timeout fire on the happy assertion below if the mock ever
   * regresses, which produces a clearer failure than asserting the
   * error node is absent.
   */
  async validateCredentials(): Promise<void> {
    await this.page
      .getByTestId(AtlassianDialogLocators.VALIDATE_BUTTON)
      .click();
    await expect(
      this.page.getByTestId(AtlassianDialogLocators.VALIDATION_OK),
    ).toBeVisible({ timeout: VALIDATION_VISIBLE_TIMEOUT_MS });
  }

  /**
   * Confirm the dialog. After `atlassian_sources_add` resolves the
   * dialog is torn down by the parent (`SourcesSidebar` sets
   * `addAtlassianOpen = false` on its `onAdded`), so we wait for
   * the dialog node to disappear as the success signal.
   */
  async submit(): Promise<void> {
    const dialog = this.page.getByTestId(AtlassianDialogLocators.DIALOG);
    await dialog
      .getByRole("button", { name: AtlassianDialogLocators.SUBMIT_BUTTON_NAME })
      .click();
    await expect(dialog).toBeHidden();
  }

  /**
   * Assert the normalised-URL hint shows the canonical origin form
   * of the workspace fixture. This is the user-visible proof that
   * `normaliseWorkspaceUrl` turned `dayseam-e2e` into
   * `https://dayseam-e2e.atlassian.net` before the IPC fired.
   */
  async expectNormalisedWorkspaceUrl(): Promise<void> {
    await expect(
      this.page.getByTestId(AtlassianDialogLocators.URL_NORMALISED),
    ).toContainText(CATALOGUE.atlassian.workspaceUrl);
  }

  private async setJira(enabled: boolean): Promise<void> {
    const box = this.page.getByTestId(AtlassianDialogLocators.ENABLE_JIRA);
    if (enabled) await box.check();
    else await box.uncheck();
  }

  private async setConfluence(enabled: boolean): Promise<void> {
    const box = this.page.getByTestId(
      AtlassianDialogLocators.ENABLE_CONFLUENCE,
    );
    if (enabled) await box.check();
    else await box.uncheck();
  }
}
