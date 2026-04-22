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

// DAY-90 TST-v0.2-02 — count-aware scenario steps. The original
// `the draft contains "…"` step passes if the substring appears
// anywhere inside the streaming preview, including inside a
// tooltip or a section heading. These two steps scope the
// assertion to a specific `data-section` id and (for the count
// variant) assert exactly N bullets — catching the
// DOG-v0.2-04-class bug where the section heading renders but the
// bullets under it silently stop arriving.
Then(
  'the {string} section contains {int} bullets',
  async ({ pages }, sectionId: string, count: number) => {
    await pages.report.expectSectionBulletCount(sectionId, count);
  },
);

Then(
  'the {string} section contains the bullet {string}',
  async ({ pages }, sectionId: string, text: string) => {
    await pages.report.expectSectionContainsBullet(sectionId, text);
  },
);
