import { render, act } from "@testing-library/react";
import { StrictMode } from "react";
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
    vi.restoreAllMocks();
    localStorage.clear();
    document.getElementById("splash")?.remove();
  });

  it("is hidden and removed once App has mounted", async () => {
    const splash = injectSplash();
    expect(document.getElementById("splash")).toBe(splash);

    render(<App />);

    // `data-hidden` flips synchronously in the `useEffect`.
    expect(splash.getAttribute("data-hidden")).toBe("true");

    // App's child hooks (useSources / useReport / …) kick off IPC
    // listeners on mount. Those promises resolve as microtasks and
    // set state after `render()` returns; under fake timers we still
    // need to flush them inside `act` so TST-05's warning floor
    // stays at zero.
    await act(async () => {
      await Promise.resolve();
    });

    // The node removes itself after the CSS fade (220ms + buffer).
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(document.getElementById("splash")).toBeNull();
  });

  it("stays gone when App remounts under StrictMode", async () => {
    // React 18's StrictMode calls effect setups twice in dev; the
    // second `dismissSplash()` must not resurrect the node, throw,
    // or double-schedule the removal timer.
    //
    // We spy on the splash element's own `setAttribute` rather than
    // end-state or `window.setTimeout`:
    //   - End-state alone isn't enough — `splash.parentNode?.removeChild`
    //     silently no-ops on a detached node via optional chaining,
    //     so a missing re-entrancy guard still lands in the same
    //     "node is gone" world.
    //   - `window.setTimeout` is noisy — React's scheduler taps it too,
    //     so the count doesn't isolate our own contribution.
    // The guard in `dismissSplash` short-circuits *before* the
    // `splash.setAttribute("data-hidden", "true")` write, so counting
    // that write on the specific element pins the guard exactly.
    const splash = injectSplash();
    const setAttrSpy = vi.spyOn(splash, "setAttribute");

    render(
      <StrictMode>
        <App />
      </StrictMode>,
    );

    // StrictMode runs `App`'s effect twice; the second call must
    // short-circuit before touching `data-hidden` again.
    const dataHiddenWrites = setAttrSpy.mock.calls.filter(
      ([name]) => name === "data-hidden",
    );
    expect(dataHiddenWrites).toEqual([["data-hidden", "true"]]);

    // Flush App's on-mount IPC listeners (same rationale as the
    // previous test) before advancing timers.
    await act(async () => {
      await Promise.resolve();
    });

    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(document.getElementById("splash")).toBeNull();
    expect(() => {
      act(() => {
        vi.runOnlyPendingTimers();
      });
    }).not.toThrow();
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

  it("is a no-op when called again mid-fade", () => {
    // If a future caller ends up invoking `dismissSplash()` from two
    // places (e.g. a preload handler plus App's effect), the second
    // call arrives while the CSS fade is still in flight and the
    // node is still attached. The helper must skip re-flipping
    // `data-hidden` and must not schedule a second remove timer.
    //
    // End-state assertions alone (`getElementById` returns null)
    // can't catch a missing guard — a second `setTimeout` firing
    // against a detached node would silently no-op via the
    // optional-chained `removeChild`. So we spy on the splash
    // element's own `setAttribute` (which the guard sits directly
    // in front of) and assert exactly one `data-hidden` write.
    const splash = injectSplash();
    const setAttrSpy = vi.spyOn(splash, "setAttribute");

    dismissSplash();
    expect(splash.getAttribute("data-hidden")).toBe("true");
    expect(
      setAttrSpy.mock.calls.filter(([name]) => name === "data-hidden"),
    ).toEqual([["data-hidden", "true"]]);

    // Advance partway — node is still in the DOM.
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(document.getElementById("splash")).toBe(splash);

    // Re-entry during the fade.
    expect(() => dismissSplash()).not.toThrow();
    expect(splash.getAttribute("data-hidden")).toBe("true");
    // Guard check: still exactly one `data-hidden` write.
    expect(
      setAttrSpy.mock.calls.filter(([name]) => name === "data-hidden"),
    ).toEqual([["data-hidden", "true"]]);

    // The original timer completes and removes the node exactly once.
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(document.getElementById("splash")).toBeNull();

    // Any stray timers left over (from a hypothetical double-schedule)
    // would fail jsdom when they fire against the detached node.
    expect(() => {
      act(() => {
        vi.runOnlyPendingTimers();
      });
    }).not.toThrow();
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
