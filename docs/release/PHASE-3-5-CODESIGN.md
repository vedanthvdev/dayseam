# Phase 3.5 / v0.1.1 — Real Developer ID codesign + notarytool

**Status:** open, tracked as a Phase 3.5 follow-up to
[`docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md`](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md).
This doc is the living spec the follow-up PR will execute and
eventually supersede; it is referenced from the v0.1.0 release notes
([Task 9](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#task-9-v010-capstone--flip-version-first-tagged-github-release)),
the [unsigned-first-run README](./UNSIGNED-FIRST-RUN.md), the Phase 3
plan itself ([Task 6](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#task-6-release-engineering--universal-dmg-github-release-workflow-gatekeeper-bypass-readme)),
and the [Phase 3 review doc](../review/phase-3-review.md) (once
Task 8 lands) so a reader of any one of those four artefacts can
find it.

## Why this is deferred, not dropped

v0.1.0 is the first binary a stranger can download. Codesigning is
the difference between "right-click → Open, click through a warning,
never see it again" (what v0.1.0 ships) and "double-click, it just
runs" (what a codesigned build does). The former is a documented
inconvenience; the latter requires:

- An Apple Developer Program membership (paid, annual, tied to a
  real legal entity / individual).
- A Developer ID Application certificate issued against that
  membership, installed in the build machine's Keychain.
- An App-Specific Password on the associated Apple ID, passed to
  `notarytool`.
- Integration of `notarytool submit … --wait` + `stapler staple` into
  the release workflow, with the output of each step gated on the
  previous one.

None of that is blocking on the *engineering* in Task 6; it is
blocking on the *paperwork* (Apple Developer account provisioning,
legal-entity decisions). Rather than let the engineering slip while
that resolves, v0.1.0 ships unsigned with a documented
Gatekeeper-bypass path, and this issue holds the slot for the real
codesign path.

## What's needed

### Apple-side prerequisites (one-time, human work)

1. **Apple Developer Program membership** enrolled and active. Link
   the membership to the repo owner's Apple ID. Record the Team ID
   (10-character alphanumeric; shown in the Account → Membership
   page) in the repo's release notes somewhere a future maintainer
   can find it.
2. **Developer ID Application certificate** generated via
   `Xcode → Preferences → Accounts → Manage Certificates` (or the
   Developer portal if the build machine doesn't have Xcode). Export
   the certificate as a `.p12` with a strong passphrase; the `.p12`
   and its passphrase are what the GitHub Actions runner needs.
3. **App-Specific Password** generated at
   `appleid.apple.com → Sign-In and Security → App-Specific
   Passwords`. Label it `Dayseam notarytool` so a future audit trail
   names it.

### GitHub Actions secrets

Add the following repo-level secrets (Settings → Secrets and
variables → Actions):

- `APPLE_CERTIFICATE_BASE64` — the `.p12` from step 2 above,
  base64-encoded (`base64 -i dayseam-dev-id.p12 | pbcopy` on macOS).
- `APPLE_CERTIFICATE_PASSWORD` — the `.p12` passphrase.
- `APPLE_TEAM_ID` — the 10-character Team ID from step 1.
- `APPLE_ID` — the Apple ID email the membership is on.
- `APPLE_APP_SPECIFIC_PASSWORD` — the app-specific password from
  step 3.

All five should be scoped to the `release` environment (create one
with branch-protection rules restricting it to `master` and tagged
refs) so a PR from a fork cannot exfiltrate them via workflow logs.

### Release workflow changes

Inside the existing macOS build job in `.github/workflows/release.yml`,
insert the codesign + notarize steps **between** the `build-dmg.sh`
step and the GitHub Release upload step:

```yaml
- name: Import signing certificate
  env:
    CERT_BASE64: ${{ secrets.APPLE_CERTIFICATE_BASE64 }}
    CERT_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
  run: |
    echo "$CERT_BASE64" | base64 --decode > /tmp/cert.p12
    security create-keychain -p runner build.keychain
    security default-keychain -s build.keychain
    security unlock-keychain -p runner build.keychain
    security import /tmp/cert.p12 -k build.keychain \
      -P "$CERT_PASSWORD" -T /usr/bin/codesign
    security set-key-partition-list -S apple-tool:,apple: \
      -s -k runner build.keychain
    rm /tmp/cert.p12

- name: Codesign the .app (hardened runtime, timestamped)
  env:
    TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
  run: |
    APP="apps/desktop/src-tauri/target/universal-apple-darwin/release/bundle/macos/Dayseam.app"
    codesign --deep --force --options runtime --timestamp \
      --sign "Developer ID Application: <Legal Name> ($TEAM_ID)" \
      "$APP"
    codesign --verify --deep --strict --verbose=2 "$APP"

- name: Codesign the .dmg
  env:
    TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
  run: |
    DMG="apps/desktop/src-tauri/target/universal-apple-darwin/release/bundle/dmg/Dayseam-v${VERSION}.dmg"
    codesign --force --timestamp \
      --sign "Developer ID Application: <Legal Name> ($TEAM_ID)" \
      "$DMG"

- name: Notarize the .dmg
  env:
    APPLE_ID: ${{ secrets.APPLE_ID }}
    APPLE_PASSWORD: ${{ secrets.APPLE_APP_SPECIFIC_PASSWORD }}
    TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
  run: |
    DMG="apps/desktop/src-tauri/target/universal-apple-darwin/release/bundle/dmg/Dayseam-v${VERSION}.dmg"
    xcrun notarytool submit "$DMG" \
      --apple-id "$APPLE_ID" \
      --team-id "$TEAM_ID" \
      --password "$APPLE_PASSWORD" \
      --wait
    xcrun stapler staple "$DMG"
    spctl --assess --type open --context context:primary-signature \
      --verbose "$DMG"
```

The `spctl --assess` call at the end is the pass/fail gate: it
returns the same verdict Gatekeeper gives on a user's Mac, so a green
`spctl` is a green real-world install.

### Tauri config changes

`apps/desktop/src-tauri/tauri.conf.json` gains:

```json
"bundle": {
  "macOS": {
    "hardenedRuntime": true,
    "entitlements": "entitlements.plist"
  }
}
```

Plus a new `apps/desktop/src-tauri/entitlements.plist` enabling only
the entitlements Dayseam actually uses (file I/O under the user's
home dir, Keychain access for the PAT store). The entitlements file
is committed to the repo so the exact security posture is reviewable.

### Docs to flip

When this issue closes, the following artefacts are updated in the
same PR:

1. [`docs/release/UNSIGNED-FIRST-RUN.md`](./UNSIGNED-FIRST-RUN.md) —
   deprecate in favour of a new `SIGNED-INSTALL.md` (or delete
   outright, with a redirect note).
2. [`README.md`](../../README.md) install section — drop the
   Gatekeeper-bypass callout.
3. The v0.1.0 release notes in the previous GitHub Release — edit to
   add a "superseded by v0.1.1 (codesigned)" line.
4. This doc — replaced by `docs/release/CODESIGN.md` describing the
   live signing posture (keychain management, rotation cadence,
   certificate expiry handling).
5. The Phase 3 review doc — update the PHASE-3-NEW release-
   engineering / signed-artefact integrity lens row with the new
   resolution evidence.

## Verification checklist for the Phase 3.5 PR

- [ ] All five Actions secrets set in the `release` environment.
- [ ] `release` environment branch-protection rule restricts runs to
      `master` and tagged refs.
- [ ] `codesign --verify --deep --strict` passes on the `.app`.
- [ ] `codesign --verify` passes on the `.dmg`.
- [ ] `xcrun notarytool submit … --wait` returns `Accepted`.
- [ ] `xcrun stapler staple` succeeds.
- [ ] `spctl --assess --type open` returns `accepted` against the
      stapled `.dmg`.
- [ ] On a fresh Mac, double-clicking the stapled `.dmg` runs the
      app without any Gatekeeper warning. Recording attached to the
      review comment.
- [ ] `docs/release/UNSIGNED-FIRST-RUN.md` and the README install
      section are updated to reflect the signed posture.
- [ ] The Apple Developer cert expiry date is recorded somewhere
      the next maintainer can find it (release notes, pinned issue,
      repo wiki — not in the repo itself, since it shifts annually).

## What would change the resolution

- **Expired certificate:** renewing the cert re-runs the Keychain
  import step with a fresh `.p12`; the secret names don't change.
- **Apple tightens notarization requirements:** the `notarytool`
  invocation may need additional flags (e.g.
  `--webhook` for async completion, or new entitlement rules); this
  doc is the place to record the updated invocation.
- **Cross-platform expansion (Windows, Linux):** a sibling
  `docs/release/CODESIGN-WINDOWS.md` lands at the same time,
  following the same pattern.
