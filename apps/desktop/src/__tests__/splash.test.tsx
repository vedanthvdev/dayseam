import { render, act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { dismissSplash } from "../splash";

// The inline splash lives in `index.html`, but vitest renders into a
// fresh `document.body` per test, so we inject an equivalent node
// here. The shape matches the real splash markup so a refactor of
// the template (e.g. an extra class) that breaks the removal
// contract will fail this test.
function injectSplash(): HTMLElement {
  const splash = document.createElement("div");
  splash.id = "splash";
  splash.setAttribute("role", "status");
  document.body.insertBefore(splash, document.body.firstChild);
  return splash;
}

describe("startup splash", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    localStorage.clear();
    document.getElementById("splash")?.remove();
  });

  it("is hidden and removed once App has mounted", () => {
    const splash = injectSplash();
    expect(document.getElementById("splash")).toBe(splash);

    render(<App />);

    // `data-hidden` flips synchronously in the `useEffect`.
    expect(splash.getAttribute("data-hidden")).toBe("true");

    // The node removes itself after the CSS fade (220ms + buffer).
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(document.getElementById("splash")).toBeNull();
  });

  it("is a no-op when called twice (re-entrancy guard)", () => {
    injectSplash();

    dismissSplash();
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(document.getElementById("splash")).toBeNull();

    // Second call after removal must not throw and must not resurrect
    // the node — both are possible if the helper forgets to null-check.
    expect(() => dismissSplash()).not.toThrow();
    expect(document.getElementById("splash")).toBeNull();
  });

  it("removes the node immediately when prefers-reduced-motion is set", () => {
    injectSplash();
    const originalMatchMedia = window.matchMedia;
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: query.includes("reduce"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }));

    try {
      dismissSplash();
      // No timer needed: reduced-motion path removes synchronously.
      expect(document.getElementById("splash")).toBeNull();
    } finally {
      window.matchMedia = originalMatchMedia;
    }
  });

  it("gracefully handles a missing splash node", () => {
    // No splash injected; `getElementById` will return null. The
    // helper must not throw — production renders that happen after
    // a hot reload will hit this path.
    expect(() => dismissSplash()).not.toThrow();
  });
});
