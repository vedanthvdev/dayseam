// BDD steps for the generate-report portion of the happy path.
// These drive the renderer-side `useReport.generate` hook (IPC
// invoke + progress `Channel` + `report:completed` window event) by
// proxy: clicking Generate is enough, everything else is
// orchestrated by the mock in `fixtures/runtime/tauri-mock-init.ts`.

import { Then, When } from "../../../fixtures/base-fixtures";

When("I generate a report", async ({ pages }) => {
  await pages.report.clickGenerate();
});

Then("the streaming preview shows the completed draft", async ({ pages }) => {
  await pages.report.expectDraftVisible();
});

Then("the draft contains {string}", async ({ pages }, text: string) => {
  await pages.report.expectDraftContains(text);
});
