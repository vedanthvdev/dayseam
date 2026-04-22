// Page object for the Add-GitHub-source flow.
//
// The entry point lives on the main-screen sidebar ("Add source" →
// "Add GitHub source"); the dialog itself is the React component
// `AddGithubSourceDialog`. The DAY-99 scenarios drive this surface
// end-to-end: open the dialog, paste the API base URL / PAT from
// the fixture catalogue, run the `github_validate_credentials`
// probe, then submit — which issues `github_sources_add` and
// returns the fresh row that `sources_list` subsequently reflects.
//
// Mirrors the shape of `AtlassianDialogPage` so a reader jumping
// between connectors sees the same affordances in the same order.

import { expect } from "@playwright/test";
import { CATALOGUE } from "../../fixtures/runtime/catalogue";
import { BasePage } from "../base-page";
import { GithubDialogLocators } from "./github-dialog-locators";

// Generous budget for the validate-button round-trip: the mock
// resolves synchronously, but the React state machine flips through
// `checking` → `ok` and renders the confirmation ribbon on the next
// microtask. Matches the corresponding Atlassian constant.
const VALIDATION_VISIBLE_TIMEOUT_MS = 30_000;

export class GithubDialogPage extends BasePage {
  async openFromSidebar(): Promise<void> {
    await this.page
      .getByTestId(GithubDialogLocators.SIDEBAR_ADD_MENU_TRIGGER)
      .click();
    await this.page
      .getByTestId(GithubDialogLocators.SIDEBAR_ADD_MENU_GITHUB)
      .click();
    await expect(
      this.page.getByTestId(GithubDialogLocators.DIALOG),
    ).toBeVisible();
  }

  /**
   * Fill the PAT field from the fixture. The API base URL is
   * pre-filled by the dialog to `https://api.github.com/`, so the
   * happy-path scenarios only need to supply the token. A scenario
   * that wants to exercise GitHub Enterprise can `overrideApiBaseUrl`
   * before this call.
   */
  async fillCredentialsFromFixture(): Promise<void> {
    await this.page
      .getByTestId(GithubDialogLocators.PAT)
      .fill(CATALOGUE.github.pat);
  }

  /**
   * Click Validate and wait for the `✓ Connected as …` ribbon —
   * the gate the dialog enforces before enabling the `Add source`
   * button. Failing validation surfaces as a
   * `add-github-validation-error` node; we let the Playwright
   * timeout fire on the happy assertion below if the mock ever
   * regresses, which produces a clearer failure than asserting the
   * error node is absent.
   */
  async validateCredentials(): Promise<void> {
    await this.page
      .getByTestId(GithubDialogLocators.VALIDATE_BUTTON)
      .click();
    await expect(
      this.page.getByTestId(GithubDialogLocators.VALIDATION_OK),
    ).toBeVisible({ timeout: VALIDATION_VISIBLE_TIMEOUT_MS });
  }

  /**
   * DAY-99 validate-edit invariant. Edit the PAT field after a
   * successful validate. The dialog's `useEffect` on `[url, pat]`
   * must drop the cached `ok` ribbon, which this method asserts
   * inline so the feature file reads one step instead of a step
   * plus a separate status check.
   *
   * We edit the PAT (not the URL) because the URL is normalised
   * asynchronously and the ribbon flip is racy against the
   * debounce; the PAT's `onChange` → `setState` path is synchronous
   * and the invalidation fires on the next render.
   */
  async editPatAndExpectValidationDropped(): Promise<void> {
    await this.page
      .getByTestId(GithubDialogLocators.PAT)
      .fill(`${CATALOGUE.github.pat}-edited`);
    await expect(
      this.page.getByTestId(GithubDialogLocators.VALIDATION_OK),
    ).toBeHidden();
  }

  /**
   * Confirm the dialog. After `github_sources_add` resolves the
   * dialog is torn down by the parent (`SourcesSidebar` sets
   * `addGithubOpen = false` on its `onAdded`), so we wait for the
   * dialog node to disappear as the success signal.
   */
  async submit(): Promise<void> {
    const dialog = this.page.getByTestId(GithubDialogLocators.DIALOG);
    await dialog
      .getByRole("button", { name: GithubDialogLocators.SUBMIT_BUTTON_NAME })
      .click();
    await expect(dialog).toBeHidden();
  }

  /**
   * Assert the normalised-URL hint shows the canonical form of the
   * API base URL fixture. User-visible proof that
   * `normaliseGithubApiBaseUrl` produced the trailing-slash form
   * before the IPC fired.
   */
  async expectNormalisedApiBaseUrl(): Promise<void> {
    await expect(
      this.page.getByTestId(GithubDialogLocators.URL_NORMALISED),
    ).toContainText(CATALOGUE.github.apiBaseUrl);
  }
}
