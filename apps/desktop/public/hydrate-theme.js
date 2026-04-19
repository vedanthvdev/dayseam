// Pre-paint theme hydration. Loaded synchronously from `<head>` in
// `apps/desktop/index.html` via
// `<script src="/hydrate-theme.js"></script>` so it runs BEFORE the
// inline splash CSS is parsed and therefore before the first paint.
//
// Kept as plain JS in `public/` on purpose:
// - `public/` files are copied verbatim by Vite; no TS transform, no
//   module graph, no hash in the filename, which means `index.html`
//   can reference it with a stable path.
// - Non-module `<script src>` is parser-blocking, unlike
//   `<script type="module" src>` which defers. We need synchronous
//   execution to avoid a light-mode flash for dark-mode users.
// - Being same-origin, it satisfies `script-src 'self'` in
//   `tauri.conf.json` with zero CSP-hash plumbing.
//
// The behavior here is duplicated from
// `apps/desktop/src/theme/theme-logic.ts` (which `ThemeProvider` uses
// once React is mounted). Parity is enforced at test time —
// `apps/desktop/src/__tests__/hydrate-theme.test.ts` loads this file
// from disk and cross-checks it against `resolveTheme` /
// `applyResolvedTheme` for every permutation of the input space. If
// you change the storage key or resolution logic here, change it in
// `theme-logic.ts` too — the parity test fails the suite otherwise.
(function applyInitialTheme() {
  var resolved = "light";
  try {
    var stored = null;
    try {
      stored = window.localStorage.getItem("dayseam:theme");
    } catch (_storageErr) {
      // localStorage can throw in private-browsing or restricted
      // Tauri contexts; fall through with `stored = null`.
    }
    var valid = stored === "light" || stored === "dark" || stored === "system";
    var theme = valid ? stored : "system";
    if (theme === "system") {
      try {
        resolved = window.matchMedia("(prefers-color-scheme: dark)").matches
          ? "dark"
          : "light";
      } catch (_mediaErr) {
        // Some WebView builds reject `matchMedia`; match the CSS
        // "no preference = light" default.
        resolved = "light";
      }
    } else {
      resolved = theme;
    }
  } catch (_err) {
    // Belt-and-braces: any other failure must still produce a
    // painted theme so the splash doesn't sit on an unstyled <html>.
    resolved = "light";
  }
  // Write outside the try/catch so the fallback always reaches the
  // DOM even if something exotic upstream threw. This matches
  // `applyResolvedTheme` in `src/theme/theme-logic.ts` — the parity
  // test in `__tests__/hydrate-theme.test.ts` fails if the two
  // disagree on the end state.
  try {
    var root = document.documentElement;
    // Order matches `applyResolvedTheme` in `src/theme/theme-logic.ts`
    // (classList, attribute, then color-scheme) so the parity claim
    // is literal at the side-effect level, not just at end-state.
    // `color-scheme` teaches the WebView to draw native form
    // controls (date picker popover, scrollbars) in the same mode.
    root.classList.toggle("dark", resolved === "dark");
    root.setAttribute("data-theme", resolved);
    root.style.colorScheme = resolved;
  } catch (_domErr) {
    // If we can't touch `document`, the page isn't going to render
    // anyway; nothing actionable here.
  }
})();
