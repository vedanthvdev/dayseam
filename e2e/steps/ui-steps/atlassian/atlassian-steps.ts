// BDD steps for the Add-Atlassian-source happy-path scenarios.
//
// Each When/Then is a one-liner that delegates to the
// `AtlassianDialogPage` page object; the `Then …` assertions for
// draft bullets resolve against the catalogue-seeded Atlassian
// bullet strings so a drift between "what the mock serves" and
// "what the scenario expects" is a compile-time / fixture-pinned
// change, not a silently-passing test.

import { expect } from "@playwright/test";
import { Then, When } from "../../../fixtures/base-fixtures";
import { CATALOGUE } from "../../../fixtures/runtime/catalogue";
import { ReportLocators } from "../../../page-objects/report/report-locators";

When("I open the Add Atlassian source dialog", async ({ pages }) => {
  await pages.atlassian.openFromSidebar();
});

When("I select only the Jira product", async ({ pages }) => {
  await pages.atlassian.selectOnlyJira();
});

When("I select only the Confluence product", async ({ pages }) => {
  await pages.atlassian.selectOnlyConfluence();
});

When("I select both Atlassian products", async ({ pages }) => {
  await pages.atlassian.selectBothProducts();
});

When("I fill the Atlassian credentials from the fixture", async ({ pages }) => {
  await pages.atlassian.fillCredentialsFromFixture();
});

When("I validate the Atlassian credentials", async ({ pages }) => {
  await pages.atlassian.validateCredentials();
});

When("I confirm the Add Atlassian dialog", async ({ pages }) => {
  await pages.atlassian.submit();
});

Then(
  "the workspace URL hint shows the normalised origin",
  async ({ pages }) => {
    await pages.atlassian.expectNormalisedWorkspaceUrl();
  },
);

Then("the draft contains the Atlassian Jira bullet", async ({ pages }) => {
  // Route through `expectDraftContains` (on `ReportPage`) so the
  // bullet assertion keeps sharing the 30s `toContainText` budget
  // and targets the same `streaming-preview-draft` surface every
  // other draft assertion does.
  await pages.report.expectDraftContains(
    CATALOGUE.draft.atlassianJiraBullet,
  );
});

Then(
  "the draft contains the Atlassian Confluence bullet",
  async ({ pages }) => {
    await pages.report.expectDraftContains(
      CATALOGUE.draft.atlassianConfluenceBullet,
    );
  },
);

Then(
  "the draft shows a Jira and a Confluence bullet in the Completed section",
  async ({ page }) => {
    // Stronger "both present together" assertion the
    // `@atlassian-both` scenario uses: scope to the draft node and
    // require both bullet strings inside the same region. Using the
    // scoped locator (rather than a page-wide substring probe)
    // guards against a future redesign that splits Jira and
    // Confluence bullets across unrelated sections.
    const draft = page.getByTestId(ReportLocators.STREAMING_PREVIEW_DRAFT);
    await expect(draft).toContainText(CATALOGUE.draft.atlassianJiraBullet);
    await expect(draft).toContainText(
      CATALOGUE.draft.atlassianConfluenceBullet,
    );
  },
);
