// `data-testid` handles on the main application shell — the chrome
// the user lands on once onboarding is complete. Kept as a `const`
// object (rather than raw strings in the page object) so a rename on
// the React side is a single search-and-replace here, and the step
// definitions stay free of stringly-typed selectors.

export const AppShellLocators = {
  GENERATE_BUTTON: "action-row-generate",
} as const;
