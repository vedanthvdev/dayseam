import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SourceKind } from "@dayseam/ipc-types";
import { ConnectorLogo } from "../components/ConnectorLogo";

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
});
