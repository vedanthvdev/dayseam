// `data-testid` handles (and role-name regexes) for the Save Report
// dialog. Fixture ids are *not* defined here — they live in
// `fixtures/runtime/catalogue.ts` and are imported where needed so
// the mock and the page object never disagree on which sink the
// scenario is targeting.

export const SaveDialogLocators = {
  OPEN_SAVE_BUTTON_NAME: /save report/i,
  DIALOG: "save-report-dialog",
  SINK_RADIO_PREFIX: "save-sink-",
  CONFIRM_SAVE_BUTTON_NAME: /^save$/i,
  RECEIPTS: "save-report-receipts",
} as const;
