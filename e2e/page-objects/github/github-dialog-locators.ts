// `data-testid` handles on the Add-GitHub-source surface. The
// source-add menu entry-point lives on the main-screen sidebar; the
// dialog itself is rendered by
// `apps/desktop/src/features/sources/AddGithubSourceDialog.tsx`.
// Selectors live here rather than in steps so a React-side rename
// is a one-file edit and the steps stay selector-free — mirrors the
// shape of `atlassian-dialog-locators.ts`.

export const GithubDialogLocators = {
  SIDEBAR_ADD_MENU_TRIGGER: "sources-add-menu-trigger",
  SIDEBAR_ADD_MENU_GITHUB: "sources-add-menu-github",
  DIALOG: "add-github-dialog",
  API_BASE_URL: "add-github-api-base-url",
  URL_NORMALISED: "add-github-url-normalised",
  URL_INVALID: "add-github-url-invalid",
  PAT: "add-github-pat",
  LABEL: "add-github-label",
  VALIDATE_BUTTON: "add-github-validate",
  VALIDATION_OK: "add-github-validation-ok",
  VALIDATION_ERROR: "add-github-validation-error",
  SUBMIT_BUTTON_NAME: /^add source$/i,
} as const;
