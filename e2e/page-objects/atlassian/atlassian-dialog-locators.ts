// `data-testid` handles on the Add-Atlassian-source surface. The
// source-add menu entry-point lives on the main-screen sidebar; the
// dialog itself is rendered by
// `apps/desktop/src/features/sources/AddAtlassianSourceDialog.tsx`.
// Mirroring the rest of the page-object layer, selectors live here
// rather than in steps so a React-side rename is a one-file edit
// and the steps stay selector-free.

export const AtlassianDialogLocators = {
  SIDEBAR_ADD_MENU_TRIGGER: "sources-add-menu-trigger",
  SIDEBAR_ADD_MENU_ATLASSIAN: "sources-add-menu-atlassian",
  DIALOG: "add-atlassian-dialog",
  ENABLE_JIRA: "add-atlassian-enable-jira",
  ENABLE_CONFLUENCE: "add-atlassian-enable-confluence",
  WORKSPACE_URL: "add-atlassian-workspace-url",
  URL_NORMALISED: "add-atlassian-url-normalised",
  URL_INVALID: "add-atlassian-url-invalid",
  EMAIL: "add-atlassian-email",
  API_TOKEN: "add-atlassian-api-token",
  VALIDATE_BUTTON: "add-atlassian-validate",
  VALIDATION_OK: "add-atlassian-validation-ok",
  VALIDATION_ERROR: "add-atlassian-validation-error",
  SUBMIT_BUTTON_NAME: /^add source$/i,
} as const;
