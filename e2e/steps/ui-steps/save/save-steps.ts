// BDD steps for the save-report portion of the happy path.
//
// `I save the draft to the configured markdown sink` bundles the
// three clicks (open dialog -> pick sink -> confirm) into a single
// scenario line so the feature file reads as a user intent and not
// as a UI walkthrough. The IPC assertion is exposed separately so a
// scenario can opt in to it (it's the strongest regression signal
// in the suite — it catches contract drift between the renderer
// wiring and the Rust-side `report_save` command).

import { expect } from "@playwright/test";
import { Then, When } from "../../../fixtures/base-fixtures";
import { CATALOGUE } from "../../../fixtures/runtime/catalogue";

When("I save the draft to the configured markdown sink", async ({ pages }) => {
  await pages.save.openDialog();
  await pages.save.selectConfiguredSink();
  await pages.save.confirm();
});

Then(
  "a save receipt is shown listing {string}",
  async ({ pages }, path: string) => {
    await pages.save.expectReceiptContains(path);
  },
);

Then(
  "the captured save IPC call targets the configured sink at {string}",
  async ({ tauriMock }, destination: string) => {
    const calls = await tauriMock.capturedSaveCalls();
    expect(
      calls,
      `expected exactly one captured report_save call; got ${calls.length}`,
    ).toHaveLength(1);
    expect(calls[0]).toMatchObject({
      draftId: CATALOGUE.ids.draft,
      sinkId: CATALOGUE.ids.sink,
      destinations: [destination],
    });
  },
);
