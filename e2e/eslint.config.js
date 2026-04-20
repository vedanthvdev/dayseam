import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";

// Scoped ESLint config for the `@dayseam/e2e` package.
//
// We deliberately do not inherit from `apps/desktop/eslint.config.js`
// because that config pulls in React + react-refresh rules that are
// irrelevant here (no JSX, no HMR boundary). Keeping the e2e config
// self-contained also means adding a new test file can't
// accidentally flip a React-ecosystem plugin's version requirement
// on the desktop app.
export default tseslint.config(
  {
    // Playwright writes its HTML report + per-run traces/screenshots
    // into these dirs on every run. `playwright-bdd` writes compiled
    // `.spec.ts` files for every `.feature` into `.features-gen/` on
    // every run. All three are generated artefacts, not hand-written
    // code.
    ignores: [".features-gen", "report", "test-results", "node_modules"],
  },
  {
    files: ["**/*.ts"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
);
