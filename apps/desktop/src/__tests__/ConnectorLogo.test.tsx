import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SourceKind } from "@dayseam/ipc-types";
import { connectorAccent, ConnectorLogo } from "../components/ConnectorLogo";

// DAY-159. Coverage for the inline brand-mark component that
// `SourceChip` (and future dialog / action-row surfaces) depend
// on. The component itself is tiny, but the invariants it locks
// down are load-bearing for the connector UX:
//
// 1. Every canonical `SourceKind` has a mark — missing an entry
//    would silently render a broken `<path d={undefined}>` and
//    ship. This test iterates over every enum value so the
//    compiler and the runtime both complain if a new kind lands
//    without a corresponding mark.
// 2. Accessibility defaults: the mark is decorative by default
//    (aria-hidden, no <title>) because callers almost always
//    render a visible text label next to it; duplicating that
//    label as alt text is the canonical redundant-alt antipattern.
//    The `labelled` escape hatch exists for icon-only contexts,
//    where the mark is the only source of identity. Both modes
//    are pinned here.
// 3. `currentColor` fill — if the mark ever hard-codes a brand
//    hex (say someone "fixes" GitLab to be orange), it stops
//    following the chip's text colour and dark-mode styles, and
//    the fix is to revert. The assertion pins the contract.

const ALL_KINDS: readonly SourceKind[] = [
  "LocalGit",
  "GitHub",
  "GitLab",
  "Jira",
  "Confluence",
];

describe("ConnectorLogo", () => {
  it.each(ALL_KINDS)("renders a mark for %s", (kind) => {
    const { getByTestId } = render(<ConnectorLogo kind={kind} />);
    const svg = getByTestId(`connector-logo-${kind}`);
    expect(svg.tagName.toLowerCase()).toBe("svg");
    expect(svg.getAttribute("fill")).toBe("currentColor");
    const path = svg.querySelector("path");
    expect(path).not.toBeNull();
    expect(path!.getAttribute("d")).toBeTruthy();
    expect(path!.getAttribute("d")!.length).toBeGreaterThan(50);
  });

  it("defaults to decorative (aria-hidden, no title, no role)", () => {
    const { getByTestId } = render(<ConnectorLogo kind="GitHub" />);
    const svg = getByTestId("connector-logo-GitHub");
    expect(svg.getAttribute("aria-hidden")).toBe("true");
    expect(svg.getAttribute("role")).toBeNull();
    expect(svg.getAttribute("aria-label")).toBeNull();
    expect(svg.querySelector("title")).toBeNull();
  });

  it("exposes a labelled <title> and role=img when `labelled` is true", () => {
    const { getByTestId } = render(<ConnectorLogo kind="Jira" labelled />);
    const svg = getByTestId("connector-logo-Jira");
    expect(svg.getAttribute("aria-hidden")).toBeNull();
    expect(svg.getAttribute("role")).toBe("img");
    expect(svg.getAttribute("aria-label")).toBe("Jira");
    const title = svg.querySelector("title");
    expect(title).not.toBeNull();
    expect(title!.textContent).toBe("Jira");
  });

  it("honours the size prop and falls back to 14px", () => {
    const { getByTestId, rerender } = render(<ConnectorLogo kind="GitLab" />);
    const svg = getByTestId("connector-logo-GitLab");
    expect(svg.getAttribute("width")).toBe("14");
    expect(svg.getAttribute("height")).toBe("14");
    rerender(<ConnectorLogo kind="GitLab" size={24} />);
    expect(svg.getAttribute("width")).toBe("24");
    expect(svg.getAttribute("height")).toBe("24");
  });

  it("maps LocalGit to the canonical Git mark (brand name 'Local Git repository')", () => {
    const { getByTestId } = render(
      <ConnectorLogo kind="LocalGit" labelled />,
    );
    const svg = getByTestId("connector-logo-LocalGit");
    expect(svg.querySelector("title")!.textContent).toBe(
      "Local Git repository",
    );
  });

  it("forwards className and arbitrary SVG props", () => {
    const { getByTestId } = render(
      <ConnectorLogo kind="Confluence" className="shrink-0 text-blue-500" />,
    );
    const svg = getByTestId("connector-logo-Confluence");
    expect(svg.getAttribute("class")).toContain("shrink-0");
    expect(svg.getAttribute("class")).toContain("text-blue-500");
  });

  // DAY-170. The `colored` opt-in is how the Sources sidebar, the
  // Add-source dropdown, and the Identity manager surface the one
  // deliberate splash of colour in the app. The unit-level contract
  // we pin here is:
  //
  // 1. Default (monochrome) render must not set any inline `color`
  //    style — callers rely on Tailwind's text-* classes flowing
  //    through `currentColor` untouched.
  // 2. `colored` must set a `color` via CSS `light-dark(...)` so the
  //    mark flips between light-mode and dark-mode accent hexes in
  //    lockstep with the `color-scheme` the theme provider writes to
  //    `<html>`, without the component having to subscribe to the
  //    React theme context.
  // 3. The accent hexes exposed by `connectorAccent(kind)` must be
  //    the exact pair embedded in the inline style string — this
  //    guards against an edit that silently diverges the exported
  //    helper from the rendered markup.
  it("does not set an inline color by default (monochrome)", () => {
    const { getByTestId } = render(<ConnectorLogo kind="GitHub" />);
    // The returned element is an HTMLElement in Testing Library's
    // types even though the DOM node is an SVGSVGElement at
    // runtime; reaching .style through the HTMLElement surface is
    // fine here because every DOM element has `style` regardless of
    // namespace. Casting to SVGElement triggers TS2352 because the
    // two types don't overlap in Testing Library's lib.dom surface.
    const svg = getByTestId("connector-logo-GitHub");
    expect(svg.style.color).toBe("");
    expect(svg.getAttribute("data-colored")).toBeNull();
    expect(svg.getAttribute("data-accent-light")).toBeNull();
    expect(svg.getAttribute("data-accent-dark")).toBeNull();
  });

  it("applies the brand accent pair via light-dark() when `colored`", () => {
    const { getByTestId } = render(<ConnectorLogo kind="Jira" colored />);
    const svg = getByTestId("connector-logo-Jira");
    const accent = connectorAccent("Jira");
    // JSDOM strips `color: light-dark(...)` from inline style since
    // it can't parse the function, so we can't assert the hexes
    // through `style.color` or the serialised `style` attribute. The
    // component instead exposes the resolved accent pair as
    // `data-accent-light` / `data-accent-dark` for exactly this
    // reason — one observable place for unit tests and Playwright
    // to lock down "the Jira-coloured mark is the Jira-coloured
    // mark", without reaching into CSSOM internals.
    expect(svg.getAttribute("data-colored")).toBe("true");
    expect(svg.getAttribute("data-accent-light")).toBe(accent.light);
    expect(svg.getAttribute("data-accent-dark")).toBe(accent.dark);
  });

  it("exposes every kind's accent via connectorAccent()", () => {
    for (const kind of ALL_KINDS) {
      const accent = connectorAccent(kind);
      expect(accent.light).toMatch(/^#[0-9A-F]{6}$/i);
      expect(accent.dark).toMatch(/^#[0-9A-F]{6}$/i);
    }
  });
});
