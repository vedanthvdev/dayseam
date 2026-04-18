// Helpers for dismissing the inline splash screen defined in
// `apps/desktop/index.html`.
//
// The splash is plain HTML/CSS so it paints the instant the webview
// has the document — before Vite's JS bundle has even finished
// parsing. Once React's `App` mounts we flip the node's
// `data-hidden` attribute, which kicks off the CSS fade, and then
// remove the node from the tree once the transition completes so
// it can't capture focus or screen-reader attention.
//
// Everything is defensive by design: if the splash node isn't
// present (e.g. a Vitest jsdom run, or a future index.html refactor)
// the helpers are no-ops. React must never block on the splash.

const SPLASH_ID = "splash";

// Matches the CSS transition in `index.html`. Kept as a constant so
// a stylesheet bump can't silently desync the JS timeout.
const SPLASH_FADE_MS = 220;

/**
 * Begin the splash fade and remove the node from the DOM once the
 * CSS transition has finished. Safe to call multiple times — the
 * second call is a no-op because the node is gone.
 *
 * Separated from the React tree on purpose: it runs in a
 * `useEffect` after first paint, which guarantees the user sees
 * the rendered `App` at least one frame before the splash starts
 * fading. Without that ordering you can get a flicker where the
 * splash disappears before the app has laid out.
 */
export function dismissSplash(
  doc: Document = typeof document !== "undefined"
    ? document
    : (undefined as unknown as Document),
): void {
  if (!doc) return;
  const splash = doc.getElementById(SPLASH_ID);
  if (!splash) return;

  splash.setAttribute("data-hidden", "true");

  // `matchMedia` can be absent in some test environments; guard the
  // reduced-motion branch so the helper still works there.
  const prefersReducedMotion =
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const remove = () => {
    splash.parentNode?.removeChild(splash);
  };

  if (prefersReducedMotion) {
    remove();
    return;
  }

  // `setTimeout` rather than `transitionend` because `transitionend`
  // doesn't fire on `visibility` changes in every webview and we
  // want a hard upper bound on how long the (now-invisible) node
  // lingers in the DOM.
  window.setTimeout(remove, SPLASH_FADE_MS + 20);
}
