// BDD steps that describe the state the user lands on after
// onboarding. A scenario's Background should `Given the Dayseam
// desktop app is open on the main screen` so the following steps
// start from a known surface.

import { Given } from "../../../fixtures/base-fixtures";

Given("the Dayseam desktop app is open on the main screen", async ({ pages }) => {
  await pages.appShell.openMainScreen();
});
