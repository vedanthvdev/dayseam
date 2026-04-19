import { afterEach, beforeEach, describe, expect, it } from "vitest";
// Vite's `?raw` suffix loads the file as a string at import time, so
// the test sees the exact bytes that ship to the webview. No Node
// filesystem APIs (and therefore no `@types/node` dependency) needed.
import HYDRATE_SCRIPT_SOURCE from "../../public/hydrate-theme.js?raw";
import {
  applyResolvedTheme,
  readInitialTheme,
  resolveTheme,
} from "../theme/theme-logic";
import { THEME_STORAGE_KEY, type Theme } from "../theme";

// Parity lock for the pre-paint theme hydration.
//
// `apps/desktop/public/hydrate-theme.js` is the plain-JS file that
// runs BEFORE React hydrates to avoid a light-mode flash for
// dark-mode users. `src/theme/theme-logic.ts` is the TS helpers that
// `ThemeProvider` uses once React is running. Both must agree on:
//
//   - the storage key (`THEME_STORAGE_KEY`)
//   - the set of valid stored values
//   - the fallback behaviour on bad / missing input
//   - how `"system"` resolves against `prefers-color-scheme`
//   - how the resolved theme is written to `<html>` (both the
//     `data-theme` attribute AND the `.dark` class, because Tailwind
//     relies on the latter)
//
// The test below loads `hydrate-theme.js` from disk, executes it
// against a jsdom-hosted document with a controlled localStorage +
// matchMedia, and asserts the resulting DOM state matches what the
// TS helpers produce for the same input. If someone renames the
// storage key or tweaks resolution in one file but not the other,
// this test fails and the suite goes red.

type MatchMediaStub = (query: string) => { matches: boolean };

function installMatchMedia(prefersDark: boolean): MatchMediaStub {
  const stub: MatchMediaStub = (query: string) => ({
    matches: prefersDark && query.includes("dark"),
  });
  (window as unknown as { matchMedia: MatchMediaStub }).matchMedia = stub;
  return stub;
}

function resetRoot() {
  const root = document.documentElement;
  root.removeAttribute("data-theme");
  root.classList.remove("dark");
  root.style.colorScheme = "";
}

/** Snapshot of the three channels the theme ends up on. */
function readRootTheme(): {
  dataTheme: string | null;
  dark: boolean;
  colorScheme: string;
} {
  const root = document.documentElement;
  return {
    dataTheme: root.getAttribute("data-theme"),
    dark: root.classList.contains("dark"),
    colorScheme: root.style.colorScheme,
  };
}

/** Run the shipped hydration script in-process against the live DOM. */
function runShippedHydration() {
  const factory = new Function(HYDRATE_SCRIPT_SOURCE);
  factory();
}

/** Reference implementation used by ThemeProvider once React mounts. */
function runTsHydration() {
  applyResolvedTheme(resolveTheme(readInitialTheme()));
}

describe("hydrate-theme.js parity with theme-logic.ts", () => {
  const originalMatchMedia = window.matchMedia;

  beforeEach(() => {
    localStorage.clear();
    resetRoot();
  });

  afterEach(() => {
    localStorage.clear();
    resetRoot();
    window.matchMedia = originalMatchMedia;
  });

  // Permutation grid. `null` = unset key; `"garbage"` = a legacy /
  // corrupted value we still want to tolerate.
  const storedValues: readonly (Theme | "garbage" | null)[] = [
    null,
    "garbage",
    "light",
    "dark",
    "system",
  ];
  const systemPrefs: readonly boolean[] = [false, true];

  for (const stored of storedValues) {
    for (const prefersDark of systemPrefs) {
      const label = `stored=${stored ?? "<unset>"}, prefersDark=${prefersDark}`;

      it(`matches TS hydration for ${label}`, () => {
        if (stored !== null) {
          localStorage.setItem(THEME_STORAGE_KEY, stored);
        }
        installMatchMedia(prefersDark);

        resetRoot();
        runShippedHydration();
        const fromShipped = readRootTheme();

        resetRoot();
        runTsHydration();
        const fromTs = readRootTheme();

        expect(fromShipped).toEqual(fromTs);
        // Belt-and-braces: the resulting theme must be concrete, not
        // a stray `"system"` leaking through.
        expect(fromShipped.dataTheme === "light" || fromShipped.dataTheme === "dark").toBe(
          true,
        );
      });
    }
  }

  it("references the canonical storage key", () => {
    // Guards against a silent rename on either side of the duplication.
    expect(HYDRATE_SCRIPT_SOURCE).toContain(`"${THEME_STORAGE_KEY}"`);
  });

  it("swallows localStorage exceptions and falls back to light", () => {
    const originalGetItem = Storage.prototype.getItem;
    Storage.prototype.getItem = () => {
      throw new Error("storage disabled");
    };
    installMatchMedia(false);

    try {
      resetRoot();
      expect(() => runShippedHydration()).not.toThrow();
      expect(readRootTheme()).toEqual({
        dataTheme: "light",
        dark: false,
        colorScheme: "light",
      });

      // Parity: TS helpers must agree on the same input.
      resetRoot();
      runTsHydration();
      expect(readRootTheme()).toEqual({
        dataTheme: "light",
        dark: false,
        colorScheme: "light",
      });
    } finally {
      Storage.prototype.getItem = originalGetItem;
    }
  });

  it("falls back to light when window.matchMedia is missing entirely", () => {
    // Distinct from "matchMedia throws" — some embedded WebViews
    // don't expose the API at all. Both layers probe with `typeof`
    // before calling, so both must deterministically paint light.
    const originalMatchMedia = window.matchMedia;
    // @ts-expect-error — deliberately making the API absent.
    delete window.matchMedia;

    try {
      resetRoot();
      expect(() => runShippedHydration()).not.toThrow();
      const fromShipped = readRootTheme();

      resetRoot();
      expect(() => runTsHydration()).not.toThrow();
      const fromTs = readRootTheme();

      expect(fromShipped).toEqual(fromTs);
      expect(fromShipped).toEqual({
        dataTheme: "light",
        dark: false,
        colorScheme: "light",
      });
    } finally {
      window.matchMedia = originalMatchMedia;
    }
  });

  it("falls back to light when window.localStorage is missing entirely", () => {
    // Same failure mode, but for storage. Accessing `localStorage`
    // on some restricted contexts throws a DOMException; overriding
    // the getter to throw lets us simulate that without touching
    // jsdom internals.
    const originalDescriptor = Object.getOwnPropertyDescriptor(
      window,
      "localStorage",
    );
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() {
        throw new Error("localStorage access denied");
      },
    });
    installMatchMedia(false);

    try {
      resetRoot();
      expect(() => runShippedHydration()).not.toThrow();
      const fromShipped = readRootTheme();

      resetRoot();
      expect(() => runTsHydration()).not.toThrow();
      const fromTs = readRootTheme();

      expect(fromShipped).toEqual(fromTs);
      expect(fromShipped).toEqual({
        dataTheme: "light",
        dark: false,
        colorScheme: "light",
      });
    } finally {
      if (originalDescriptor) {
        Object.defineProperty(window, "localStorage", originalDescriptor);
      }
    }
  });
});
