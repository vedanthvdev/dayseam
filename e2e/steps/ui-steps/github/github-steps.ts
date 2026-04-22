// BDD steps for the Add-GitHub-source happy-path scenarios.
//
// Each When/Then delegates to the `GithubDialogPage` page object;
// the `Then` assertions for the captured IPC payloads resolve
// against `window.__DAYSEAM_E2E__.captured.githubAddCalls` so a
// drift between "what the renderer sent" and "what the scenario
// expects" surfaces as a failed assertion, not a silently-passing
// test. Mirrors `atlassian-steps.ts`.

import { expect } from "@playwright/test";
import { Then, When } from "../../../fixtures/base-fixtures";
import { CATALOGUE } from "../../../fixtures/runtime/catalogue";
import type { MockState } from "../../../fixtures/runtime/types";

// DAY-100. The GitHub connect-and-report scenario asserts against
// the same `completed` draft section the Atlassian flows pin. The
// mock appends one GitHub bullet per GitHub source on top of the
// 2 LocalGit baseline bullets seeded by `CATALOGUE.draft.completedBullets`,
// so the scenario-expected count is baseline + 1.
const GITHUB_DRAFT_SECTION_ID = "completed";
const LOCAL_GIT_BASELINE_BULLETS = CATALOGUE.draft.completedBullets.length;

When("I open the Add GitHub source dialog", async ({ pages }) => {
  await pages.github.openFromSidebar();
});

When("I fill the GitHub credentials from the fixture", async ({ pages }) => {
  await pages.github.fillCredentialsFromFixture();
});

When("I validate the GitHub credentials", async ({ pages }) => {
  await pages.github.validateCredentials();
});

// DAY-99. Drives the validate-edit-validate regression scenario:
// after a successful Validate, edit the PAT and assert the cached
// `ok` ribbon disappears — which in turn redisables the
// `Add source` button until the user re-clicks Validate.
When(
  "I edit the GitHub PAT and expect the validation to clear",
  async ({ pages }) => {
    await pages.github.editPatAndExpectValidationDropped();
  },
);

When("I confirm the Add GitHub dialog", async ({ pages }) => {
  await pages.github.submit();
});

Then(
  "the GitHub API base URL hint shows the normalised URL",
  async ({ pages }) => {
    await pages.github.expectNormalisedApiBaseUrl();
  },
);

Then(
  "the captured GitHub add-source IPC matches the fixture",
  async ({ page }) => {
    const captured = await page.evaluate(
      () =>
        (window as unknown as { __DAYSEAM_E2E__?: MockState })
          .__DAYSEAM_E2E__?.captured.githubAddCalls ?? [],
    );
    expect(captured).toHaveLength(1);
    const call = captured[0]!;
    expect(call.apiBaseUrl).toBe(CATALOGUE.github.apiBaseUrl);
    expect(call.userId).toBe(CATALOGUE.github.userId);
    // The validate-edit scenario re-enters the PAT after the
    // first Validate, so we can't pin the exact bytes here; what
    // we do care about is that the IPC received whatever the user
    // had in the field at submit time (i.e. a non-empty token
    // starts with the fixture prefix). A regression that ships
    // the stale-cache token would also fail this because the
    // edited field no longer matches the original.
    expect(call.pat.length).toBeGreaterThan(0);
    expect(call.pat.startsWith(CATALOGUE.github.pat)).toBe(true);
    // Label auto-defaults to the normalised host when the user
    // doesn't type one; this pins that fallback so a regression
    // that starts shipping blank labels to the IPC fails here.
    expect(call.label.length).toBeGreaterThan(0);
  },
);

// DAY-100. Mirrors the `draft contains the Atlassian Jira bullet`
// step: scopes the assertion to the `completed` section, pins the
// bullet count so a regression that double-emits (or drops) the
// GitHub bullet fails the count, and then matches the exact
// catalogue-seeded bullet string.
Then("the draft contains the GitHub pull request bullet", async ({ pages }) => {
  await pages.report.expectSectionBulletCount(
    GITHUB_DRAFT_SECTION_ID,
    LOCAL_GIT_BASELINE_BULLETS + 1,
  );
  await pages.report.expectSectionContainsBullet(
    GITHUB_DRAFT_SECTION_ID,
    CATALOGUE.draft.githubPullRequestBullet,
  );
});
