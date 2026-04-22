// BDD steps for the Add-Atlassian-source happy-path scenarios.
//
// Each When/Then is a one-liner that delegates to the
// `AtlassianDialogPage` page object; the `Then …` assertions for
// draft bullets resolve against the catalogue-seeded Atlassian
// bullet strings so a drift between "what the mock serves" and
// "what the scenario expects" is a compile-time / fixture-pinned
// change, not a silently-passing test.

import { Then, When } from "../../../fixtures/base-fixtures";
import { CATALOGUE } from "../../../fixtures/runtime/catalogue";

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

// DAY-90 TST-v0.2-05. Drives the validate-edit-validate regression
// scenario: after a successful Validate, edit the email and assert
// the cached `ok` ribbon disappears — which in turn redisables the
// `Add source` button until the user re-clicks Validate.
When(
  "I edit the Atlassian email and expect the validation to clear",
  async ({ pages }) => {
    await pages.atlassian.editEmailAndExpectValidationDropped();
  },
);

When("I confirm the Add Atlassian dialog", async ({ pages }) => {
  await pages.atlassian.submit();
});

Then(
  "the workspace URL hint shows the normalised origin",
  async ({ pages }) => {
    await pages.atlassian.expectNormalisedWorkspaceUrl();
  },
);

// DAY-90 TST-v0.2-02: every Atlassian-draft assertion is now
// scoped to the `completed` section and uses either a count-aware
// `toHaveCount` or a filtered `[data-bullet]` locator, so a drift
// between "mock seeded N bullets" and "UI rendered N bullets"
// fails the assertion instead of passing on a coincidental
// substring match inside a heading or tooltip. The mock's
// `buildDraft()` (see `tauri-mock-init.ts`) emits:
//   - 2 LocalGit completed bullets (from catalogue.draft.completedBullets)
//   - + 1 Jira bullet if a Jira source is registered
//   - + 1 Confluence bullet if a Confluence source is registered
// so the expected per-scenario bullet count is 2 + product count.
const ATLASSIAN_DRAFT_SECTION_ID = "completed";
const LOCAL_GIT_BASELINE_BULLETS = CATALOGUE.draft.completedBullets.length;

Then("the draft contains the Atlassian Jira bullet", async ({ pages }) => {
  await pages.report.expectSectionBulletCount(
    ATLASSIAN_DRAFT_SECTION_ID,
    LOCAL_GIT_BASELINE_BULLETS + 1,
  );
  await pages.report.expectSectionContainsBullet(
    ATLASSIAN_DRAFT_SECTION_ID,
    CATALOGUE.draft.atlassianJiraBullet,
  );
});

Then(
  "the draft contains the Atlassian Confluence bullet",
  async ({ pages }) => {
    await pages.report.expectSectionBulletCount(
      ATLASSIAN_DRAFT_SECTION_ID,
      LOCAL_GIT_BASELINE_BULLETS + 1,
    );
    await pages.report.expectSectionContainsBullet(
      ATLASSIAN_DRAFT_SECTION_ID,
      CATALOGUE.draft.atlassianConfluenceBullet,
    );
  },
);

Then(
  "the draft shows a Jira and a Confluence bullet in the Completed section",
  async ({ pages }) => {
    // Journey A: both Atlassian products present → 2 LocalGit
    // baseline + 1 Jira + 1 Confluence = 4 bullets. A future
    // redesign that splits Jira and Confluence into separate
    // sections fails this count (the `completed` section would
    // then only hold 3); a regression that drops either Atlassian
    // bullet also fails the count.
    await pages.report.expectSectionBulletCount(
      ATLASSIAN_DRAFT_SECTION_ID,
      LOCAL_GIT_BASELINE_BULLETS + 2,
    );
    await pages.report.expectSectionContainsBullet(
      ATLASSIAN_DRAFT_SECTION_ID,
      CATALOGUE.draft.atlassianJiraBullet,
    );
    await pages.report.expectSectionContainsBullet(
      ATLASSIAN_DRAFT_SECTION_ID,
      CATALOGUE.draft.atlassianConfluenceBullet,
    );
  },
);
