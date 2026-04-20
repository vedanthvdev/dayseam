# Opening Dayseam on a Mac — first run

Dayseam v0.1.0 ships as an **unsigned** macOS application, which
means Gatekeeper (the macOS feature that warns about software from
unidentified developers) will refuse to open it on the first
attempt. This is a one-time click-through: once Gatekeeper has seen
you approve the app explicitly, every subsequent launch works
normally, no warning.

Every user has to do this exactly once. Future Dayseam releases
(tracked in [Phase 3.5 / v0.1.1](https://github.com/vedanthvdev/dayseam/issues/59))
will ship codesigned + notarized, and this whole page becomes
obsolete the moment that lands.

## The two-click path

1. **Download** `Dayseam-v0.1.0.dmg` from
   [the GitHub Releases page](https://github.com/vedanthvdev/dayseam/releases/latest)
   and double-click it to mount. Drag `Dayseam.app` into
   `/Applications` (or anywhere else on your disk; Applications is
   just convention).
2. **Open the app with a right-click**, not a double-click. In
   Finder, either right-click `Dayseam.app` and choose **Open** from
   the context menu, or Control-click the app icon and choose
   **Open**. A dialog appears that says
   *"macOS cannot verify the developer of `Dayseam`. Are you sure you
   want to open it?"* with an **Open** button.
3. **Click Open.** The app launches. From now on, double-clicking
   `Dayseam.app` works normally — macOS remembers your explicit
   approval for this specific binary.

If you double-click first, you'll get a different dialog with only a
**Move to Trash** button and no Open option. That's the deny-by-
default path; close it, then follow step 2 above (right-click →
Open) to get the approve-able dialog.

## On macOS 15 (Sequoia) and later

Sequoia tightened Gatekeeper: the right-click-Open path still works,
but it may show up a second dialog referring you to **System
Settings → Privacy & Security**. If that happens:

1. Close the dialog.
2. Open **System Settings** (Apple menu → System Settings).
3. Click **Privacy & Security** in the sidebar.
4. Scroll down. You'll see a line near the bottom that reads
   *"Dayseam was blocked from use because it is not from an
   identified developer."* with an **Open Anyway** button.
5. Click **Open Anyway**. You'll be prompted for your Mac password
   to confirm.
6. A new warning dialog appears with an **Open** button; click it.
   The app launches and every future double-click works.

## Why unsigned?

Signing a Mac application requires an Apple Developer Program
account, a Developer ID certificate, and a notarization submission
per release. All three are tracked as
[Phase 3.5 / v0.1.1](https://github.com/vedanthvdev/dayseam/issues/59)
and are gated on the paperwork side, not the engineering side.
v0.1.0 is the project's first public binary and we wanted it to
exist without waiting on the Apple Developer account. The trade-off
is the one-time click-through above; the binary itself is built and
packaged by the same CI workflow that runs the test suite, and the
source you'd verify against is the tagged `v0.1.0` commit in this
repo.

## Verifying the download (optional)

If you want to check that the DMG you downloaded matches the one CI
built, each GitHub Release includes a `.dmg.sha256` file alongside
the DMG. Download both, then:

```shell
shasum -a 256 Dayseam-v0.1.0.dmg
```

Compare the printed hash against the contents of
`Dayseam-v0.1.0.dmg.sha256`. If they match, the bytes you have are
the bytes CI built; if they don't, your download was corrupted or
tampered with and you should re-download.

## On updates

When v0.1.1 lands with a real codesignature, you'll be able to
download the new DMG, drag it over the old `Dayseam.app`, and
double-click to launch — no right-click-Open dance. The approval
from v0.1.0 doesn't automatically extend to v0.1.1 because macOS
keys Gatekeeper approvals to the binary's cryptographic identity,
but the signed binary won't need the approval at all.

Until then, every Dayseam release during the 0.1.x series will
walk the same two-click path.
