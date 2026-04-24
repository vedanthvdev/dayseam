import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  emitEvent,
  MockUpdate,
  mockRelaunch,
  mockUpdaterCheck,
  queueUpdaterCheck,
  resetTauriMocks,
} from "../../../__tests__/tauri-mock";
import { __clearSkippedVersionsForTests } from "../skipped-versions";
import { UpdaterBanner } from "../UpdaterBanner";
import { useUpdater } from "../useUpdater";

function Harness() {
  const state = useUpdater();
  return (
    <div>
      <span data-testid="kind">{state.status.kind}</span>
      <UpdaterBanner state={state} />
    </div>
  );
}

describe("useUpdater + UpdaterBanner", () => {
  beforeEach(() => {
    resetTauriMocks();
    __clearSkippedVersionsForTests();
  });
  afterEach(() => {
    __clearSkippedVersionsForTests();
  });

  it("renders nothing when the check returns no update", async () => {
    queueUpdaterCheck(null);
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );
    // No banner subtree means none of the tone rows rendered.
    expect(screen.queryByTestId("updater-banner-available")).toBeNull();
    expect(screen.queryByTestId("updater-banner-error")).toBeNull();
  });

  it("surfaces an available update with current + target versions", async () => {
    queueUpdaterCheck(
      new MockUpdate({
        version: "0.6.1",
        currentVersion: "0.6.0",
        body: "Bug fixes",
      }),
    );
    render(<Harness />);
    const banner = await screen.findByTestId("updater-banner-available");
    expect(banner.textContent).toContain("Dayseam 0.6.1");
    expect(banner.textContent).toContain("you have 0.6.0");
  });

  it("hides the banner once Skip this version is clicked and persists it", async () => {
    queueUpdaterCheck(
      new MockUpdate({ version: "0.6.1", currentVersion: "0.6.0" }),
    );
    const { unmount } = render(<Harness />);
    const skipBtn = await screen.findByRole("button", {
      name: /skip this version/i,
    });
    await act(async () => {
      skipBtn.click();
    });
    expect(screen.queryByTestId("updater-banner-available")).toBeNull();
    // Would-have-caught: a past version of this hook dismissed the
    // banner only in local React state, so a fresh mount re-
    // rendered the same prompt. Remounting verifies the skip hit
    // localStorage and is honoured by the next check-cycle.
    unmount();
    queueUpdaterCheck(
      new MockUpdate({ version: "0.6.1", currentVersion: "0.6.0" }),
    );
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("available"),
    );
    expect(screen.queryByTestId("updater-banner-available")).toBeNull();
  });

  it("runs the download → ready → relaunch pipeline on Install click", async () => {
    const update = new MockUpdate({
      version: "0.6.1",
      currentVersion: "0.6.0",
      downloadEvents: [
        { event: "Started", data: { contentLength: 200 } },
        { event: "Progress", data: { chunkLength: 100 } },
        { event: "Progress", data: { chunkLength: 100 } },
        { event: "Finished" },
      ],
    });
    queueUpdaterCheck(update);
    render(<Harness />);
    const installBtn = await screen.findByRole("button", {
      name: /install and restart/i,
    });
    await act(async () => {
      installBtn.click();
    });
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("ready"),
    );
    expect(screen.getByTestId("updater-banner-ready").textContent).toContain(
      "0.6.1 installed",
    );
    // `relaunch()` must fire exactly once — a missing call on macOS
    // leaves the old binary resident and the user has to quit
    // manually, which is the single UX bug this feature exists to
    // avoid.
    expect(mockRelaunch).toHaveBeenCalledTimes(1);
    expect(update.installCalls).toBe(1);
  });

  it("renders an indeterminate progress bar when Content-Length is missing", async () => {
    queueUpdaterCheck(
      new MockUpdate({
        version: "0.6.1",
        currentVersion: "0.6.0",
        downloadEvents: [
          { event: "Started", data: {} },
          { event: "Progress", data: { chunkLength: 42 } },
        ],
      }),
    );
    render(<Harness />);
    const installBtn = await screen.findByRole("button", {
      name: /install and restart/i,
    });
    await act(async () => {
      installBtn.click();
    });
    const downloading = await screen.findByTestId("updater-banner-downloading");
    // Would-have-caught: a regression that computed `NaN%` from
    // `received / null` would emit `NaN` into the label and as
    // `aria-valuenow`; we pin the copy + the lack of aria-valuenow
    // so both failure modes fail the test.
    expect(downloading.textContent).toMatch(
      /Downloading Dayseam 0\.6\.1…/,
    );
    const progress = downloading.querySelector('[role="progressbar"]');
    expect(progress).not.toBeNull();
    expect(progress?.getAttribute("aria-valuenow")).toBeNull();
  });

  it("exposes errors from the check as a retryable banner", async () => {
    queueUpdaterCheck(new Error("network down"));
    render(<Harness />);
    const banner = await screen.findByTestId("updater-banner-error");
    expect(banner.textContent).toContain("network down");
    // Retry replays the check — queue a success and click.
    queueUpdaterCheck(null);
    const retry = await screen.findByRole("button", { name: /retry/i });
    await act(async () => {
      retry.click();
    });
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );
  });

  // DAY-119: when the native "Check for Updates…" menu item is
  // clicked, the Rust setup hook emits `menu://check-for-updates`.
  // `useUpdater` must listen for that event and re-run its
  // `runCheck()` path so the user sees a fresh state transition —
  // otherwise the menu entry is inert chrome that reproduces
  // exactly the UX gap this bug was filed against ("I still don't
  // see the check for updates when I click on the dayseam on the
  // top left"). The test pins the behaviour by:
  //   1. returning "up-to-date" on mount,
  //   2. queueing a *new* available update,
  //   3. firing the menu event, and
  //   4. asserting the banner flips from clean to "available".
  // A fix-revert that removes the `listen()` block leaves the
  // banner stuck on "up-to-date" and the test fails.
  it("re-runs the update check when the native menu item fires the event", async () => {
    queueUpdaterCheck(null);
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );

    queueUpdaterCheck(
      new MockUpdate({
        version: "0.6.3",
        currentVersion: "0.6.2",
        body: "Hot patch",
      }),
    );
    await act(async () => {
      emitEvent("menu://check-for-updates", null);
    });

    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("available"),
    );
    const banner = await screen.findByTestId("updater-banner-available");
    expect(banner.textContent).toContain("Dayseam 0.6.3");
  });

  // DAY-122 / C-5 regression: if a first `check()` resolves to an
  // available `Update` and a later re-check (menu item, manual
  // retry, post-install relaunch failure path) resolves to `null`,
  // the first handle must be released. Pre-C-5, `runCheck` only
  // swapped `updateRef.current` when a *new* `Update` arrived, so
  // a null resolution silently left the prior resource open and
  // leaked one Tauri bridge handle per poll.
  it("closes the stale Update resource when a re-check resolves to null", async () => {
    const first = new MockUpdate({
      version: "0.6.3",
      currentVersion: "0.6.2",
    });
    queueUpdaterCheck(first);
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("available"),
    );
    expect(first.closeCalls).toBe(0);

    // Second check: no update available. The hook must close the
    // handle it captured from the first check before flipping to
    // "up-to-date", not after an unmount that may never come in a
    // long-lived session.
    queueUpdaterCheck(null);
    await act(async () => {
      emitEvent("menu://check-for-updates", null);
    });
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );
    // Would-have-caught: the pre-C-5 hook left `closeCalls` at 0
    // here and only released the handle on component unmount. In
    // a real session (a hook that never unmounts), that's a
    // permanent resource leak.
    expect(first.closeCalls).toBe(1);
  });

  // DAY-127 #3: the silent mount-time check must *not* render the
  // verbose-only rows (Checking… / Up to date). Without this, the
  // shell would flash "Checking for updates…" on every launch — the
  // original pre-DAY-127 UX that made the banner feel busy. Pinning
  // the silent path prevents a regression where someone wires
  // verbose true by default and claims it was "for visibility".
  it("keeps the banner silent on the mount-time check when verbose has not been requested", async () => {
    queueUpdaterCheck(null);
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );
    // Neither of the verbose-only rows should appear.
    expect(screen.queryByTestId("updater-banner-checking")).toBeNull();
    expect(screen.queryByTestId("updater-banner-up-to-date")).toBeNull();
  });

  // DAY-127 #3: a user-triggered menu click drops the hook into
  // verbose mode so the banner surfaces the "Checking for updates…"
  // row during the IPC round-trip and a dismissible "Dayseam is
  // up to date." confirmation on settle. The silent mount-time
  // check must not render those rows (covered separately above)
  // so the shell doesn't flash messaging on every launch. This
  // test pins the "I clicked Check for Updates and nothing
  // happened" failure mode: before DAY-127 #3 the menu click was
  // invisible — a stray toggle that dropped the verbose rows
  // would return to that UX exactly.
  it("surfaces Checking and Up-to-date rows on a verbose menu-triggered check", async () => {
    // Mount-time check resolves silent/null so the banner is
    // clean when the menu event fires.
    queueUpdaterCheck(null);
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );
    expect(screen.queryByTestId("updater-banner-up-to-date")).toBeNull();
    expect(screen.queryByTestId("updater-banner-checking")).toBeNull();

    // Gate the verbose re-check on a pending promise so we can
    // observe the `Checking…` row mid-flight. We override
    // `mockUpdaterCheck` directly rather than go through the
    // (synchronous-valued) queue.
    let resolveCheck: (value: MockUpdate | null) => void = () => {};
    const pending = new Promise<MockUpdate | null>((resolve) => {
      resolveCheck = resolve;
    });
    mockUpdaterCheck.mockImplementationOnce(async () => pending);
    await act(async () => {
      emitEvent("menu://check-for-updates", null);
    });
    await screen.findByTestId("updater-banner-checking");

    await act(async () => {
      resolveCheck(null);
    });
    await waitFor(() =>
      expect(
        screen.queryByTestId("updater-banner-up-to-date"),
      ).not.toBeNull(),
    );
    // The Checking row must swap out — leaving it mounted would
    // double-stack the banner while we're idle again.
    expect(screen.queryByTestId("updater-banner-checking")).toBeNull();
  });

  // DAY-127 #3: back-to-back menu clicks used to fire a fresh
  // `check()` for every click, wasting IPC and (depending on the
  // updater endpoint) racing state transitions. The hook now
  // collapses repeat clicks while a check is already in flight.
  // Without a regression test, a refactor that goes back to
  // reading React state non-synchronously in the guard would
  // silently bring the duplicate calls back.
  it("collapses a second menu click while a check is still in flight", async () => {
    // Mount-time check resolves silent/null.
    queueUpdaterCheck(null);
    render(<Harness />);
    await waitFor(() =>
      expect(screen.getByTestId("kind").textContent).toBe("up-to-date"),
    );

    // First menu click: gate the IPC so the check stays in flight.
    let resolveFirst: (value: null) => void = () => {};
    const pending = new Promise<MockUpdate | null>((resolve) => {
      resolveFirst = resolve as (value: null) => void;
    });
    mockUpdaterCheck.mockImplementationOnce(async () => pending);
    const callsBeforeSecondClick = mockUpdaterCheck.mock.calls.length;
    await act(async () => {
      emitEvent("menu://check-for-updates", null);
    });
    await screen.findByTestId("updater-banner-checking");
    expect(mockUpdaterCheck.mock.calls.length).toBe(callsBeforeSecondClick + 1);

    // Second menu click while the first is still pending. The
    // guard must collapse it — no second `check()` call should
    // fire until the first resolves.
    await act(async () => {
      emitEvent("menu://check-for-updates", null);
    });
    expect(screen.queryByTestId("updater-banner-checking")).not.toBeNull();
    expect(screen.queryByTestId("updater-banner-available")).toBeNull();
    expect(mockUpdaterCheck.mock.calls.length).toBe(callsBeforeSecondClick + 1);

    // Let the first check resolve null. The banner settles on the
    // verbose up-to-date confirmation.
    await act(async () => {
      resolveFirst(null);
    });
    await waitFor(() =>
      expect(
        screen.queryByTestId("updater-banner-up-to-date"),
      ).not.toBeNull(),
    );
    expect(screen.queryByTestId("updater-banner-available")).toBeNull();
  });

  it("maps download failures into the error banner without a stray relaunch", async () => {
    queueUpdaterCheck(
      new MockUpdate({
        version: "0.6.1",
        currentVersion: "0.6.0",
        downloadError: new Error("signature verify failed"),
      }),
    );
    render(<Harness />);
    const installBtn = await screen.findByRole("button", {
      name: /install and restart/i,
    });
    await act(async () => {
      installBtn.click();
    });
    const banner = await screen.findByTestId("updater-banner-error");
    expect(banner.textContent).toContain("signature verify failed");
    // Would-have-caught: a bug where the old hook always called
    // `relaunch()` after `downloadAndInstall()` resolved — which
    // would double as a silent quit on a verification failure,
    // the opposite of what we want. The guard is simple: relaunch
    // lives inside the try-block, so a thrown download error
    // skips it.
    expect(mockRelaunch).not.toHaveBeenCalled();
  });
});
