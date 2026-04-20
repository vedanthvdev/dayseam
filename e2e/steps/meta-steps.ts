// Cross-cutting BDD steps that aren't tied to a single UI surface.
// Right now this only hosts the "no console / page errors were
// captured" assertion; keeping it out of the per-page step files
// means a future scenario can opt in to the check with a single
// sentence and new cross-cutting guards (network-tab assertions,
// fixture-state assertions, etc.) land here without disturbing the
// UI step layout.

import { Then } from "../fixtures/base-fixtures";

Then(
  "no console or page errors were captured during the run",
  async ({ diagnostics }) => {
    diagnostics.assertClean();
  },
);
