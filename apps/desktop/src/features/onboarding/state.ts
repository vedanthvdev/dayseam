// Pure derivation of the first-run checklist.
//
// The plan (§2026-04-18 Phase 2 Task 7, invariant #1) calls this out as
// a pure function of `{ person, sources, identities, sinks }`: given a
// snapshot of those four inputs, decide which of the four onboarding
// steps are already done. Keeping the derivation side-effect free lets
// us exercise every visibility rule as a unit test without mounting
// React, and lets the hook layer focus on orchestrating the network
// round-trips that produce those inputs.
//
// "Pick a name" is satisfied when the self-`Person` row exists and its
// `display_name` is something other than the well-known sentinel
// `"Me"` that `PersonRepo::bootstrap_self` stamps onto the row on its
// very first creation. A user who genuinely wants to be called "Me"
// will see the checklist linger, and will need to pick any other name
// once — that's a deliberate tradeoff against introducing a separate
// `name_confirmed` flag for a single sentinel string.

import type {
  Person,
  Sink,
  Source,
  SourceIdentity,
} from "@dayseam/ipc-types";

/**
 * Stable sentinel for the self-`Person`'s `display_name` before the
 * user has picked one. Mirrors `SELF_DEFAULT_DISPLAY_NAME` in
 * `apps/desktop/src-tauri/src/ipc/commands.rs`; any drift between the
 * two is a "pick a name" step that the UI can never clear.
 */
export const SELF_DEFAULT_DISPLAY_NAME = "Me";

/** Identifier for each onboarding step. */
export type SetupChecklistItemId = "name" | "source" | "identity" | "sink";

export interface SetupChecklistItem {
  id: SetupChecklistItemId;
  /** Short imperative title, e.g. "Pick your name". */
  title: string;
  /** One-sentence explanation of why this step matters. */
  description: string;
  /** `true` once the step's condition is met. */
  done: boolean;
}

export interface SetupChecklistInputs {
  /** `null` while `persons_get_self` is still loading. */
  person: Person | null;
  sources: Source[];
  /** Source-identity rows already attached to the self-`Person`. */
  identities: SourceIdentity[];
  sinks: Sink[];
}

export interface SetupChecklistStatus {
  items: SetupChecklistItem[];
  /**
   * `true` when every item is `done`. Kept as a separate field (rather
   * than a derived `items.every(...)`) so consumers can cache the
   * boolean cheaply without re-walking the array.
   */
  complete: boolean;
}

/**
 * Pure derivation of the checklist. Returns the four items in display
 * order (name → source → identity → sink): the order reflects the
 * prerequisites, since a source is needed before identity mappings
 * make sense, and a sink is only useful once there's something to
 * write.
 */
export function deriveSetupChecklist(
  inputs: SetupChecklistInputs,
): SetupChecklistStatus {
  const nameDone =
    inputs.person !== null &&
    inputs.person.display_name.trim().length > 0 &&
    inputs.person.display_name !== SELF_DEFAULT_DISPLAY_NAME;

  const sourceDone = inputs.sources.length > 0;
  const identityDone = inputs.identities.length > 0;
  const sinkDone = inputs.sinks.length > 0;

  const items: SetupChecklistItem[] = [
    {
      id: "name",
      title: "Pick your name",
      description:
        "We label every report with your display name. `Me` is only the placeholder until you set one.",
      done: nameDone,
    },
    {
      id: "source",
      title: "Connect a source",
      description:
        "Point Dayseam at one or more folders that contain your git repos, or connect a self-hosted GitLab instance. We\u2019ll use those to build your report.",
      done: sourceDone,
    },
    {
      id: "identity",
      title: "Confirm your identity mappings",
      description:
        "Tell Dayseam which git author emails are you so reports only include *your* commits, not the whole team\u2019s.",
      done: identityDone,
    },
    {
      id: "sink",
      title: "Pick a sink",
      description:
        "Choose a folder (an Obsidian vault works) where Dayseam should save each saved report as a Markdown file.",
      done: sinkDone,
    },
  ];

  return {
    items,
    complete: items.every((i) => i.done),
  };
}
