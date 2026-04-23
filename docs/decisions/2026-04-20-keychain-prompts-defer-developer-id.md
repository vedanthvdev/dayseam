# Decision: Accept repeated Keychain prompts under ad-hoc signing; pursue Apple Developer ID as the real fix

- **Date:** 2026-04-20
- **Status:** Decided — defer structural fix
- **Owners:** Dayseam maintainers
- **Related:**
  - `apps/desktop/src-tauri/src/startup.rs` (orphan-secret audit, the
    visible boot-time probe that surfaces the prompts)
  - `crates/dayseam-secrets/src/keychain.rs` (the `KeychainStore`
    that calls into `keyring::Entry::get_password`)
  - DAY-119 ad-hoc signing + stable designated-requirement fix
    (unblocked folder-access prompts but not keychain prompts)
  - DAY-120 entitlements-plist preflight fix

## Problem

After DAY-119/DAY-120, users running macOS builds of Dayseam still
see **one Keychain "allow" prompt per configured connector** on every
launch after an upgrade. A user with GitHub + GitLab + Atlassian
configured sees three prompts; a user who has also split Jira and
Confluence across separate PATs sees four.

The user-visible bar from the DAY-121 thread was:

> If it was once when the user downloads the app first time it is fine.

So "once per install" would be acceptable; "every launch" and "once
per connector" both are not. The keychain prompt cascade became
noticeable because DAY-119/DAY-120 successfully silenced the
folder-selection prompts, leaving the keychain prompts as the loudest
remaining boot-time UX regression.

## What actually causes the prompts

macOS Keychain permissions are enforced **per item**, not per app:

1. Each Keychain item has its own ACL that records "which binary
   is allowed to read this item without prompting."
2. The ACL identifies a binary by its **code-signing Designated
   Requirement** (DR), which under ad-hoc signing is the *cdhash* —
   a content-addressed hash of the binary bits.
3. Writing a new item is silent; reading prompts whenever the
   requesting binary's DR does not match the ACL.
4. When the user clicks **Always Allow**, macOS appends the current
   binary's DR to that one item's ACL. That grant persists — but
   only for that exact binary.
5. Re-signing Dayseam (every release, by construction) changes the
   cdhash. The previous grant no longer applies, and the item
   prompts again on the first read.

Dayseam currently stores tokens under three distinct Keychain
*services* — `dayseam.github`, `dayseam.gitlab`, `dayseam.atlassian`
— each holding one or more accounts keyed by source id or shared
slot id. The startup orphan-secret audit
(`audit_orphan_secrets`) reads every distinct `(service, account)`
pair once on launch; that is the N probes observed as N prompts on
the first launch after each release.

## Options considered

### A. Remove or defer the eager orphan-secret audit

Stops the *boot-time* prompt cascade — the audit is the only
cold-start path that touches the Keychain. The first per-connector
read then fires when the user first runs Generate against that
source, in context, and still only once per release.

Pros: tiny patch; zero migration risk; silences the worst of the UX
(boot-time surprise prompts).

Cons: user still sees one prompt per connector on first Generate
after each release. Three prompts is three prompts, whether they
fire at boot or during first use.

### B. Consolidate every token into one Keychain item

One `(service, account)` pair whose value is a JSON blob of all
configured tokens. One item → one ACL → one prompt.

Pros: 1 prompt per release, permanently. Matches the "once on first
install" bar under ad-hoc signing.

Cons:

- Migration: existing installs have 3 items. Migrating requires
  reading each (3 prompts during migration) and re-writing the
  consolidated blob.
- Every PAT rotation rewrites the full blob, introducing
  concurrency gotchas with Atlassian's two-row shared-slot model.
- Auditing which binary touched which token becomes harder (the
  OS-level records collapse into one line per launch).
- Ad-hoc signing *still* invalidates the one item on every
  release, so the underlying "re-prompt every release" problem
  persists — we've just reduced its cardinality.

### C. Pursue Apple Developer ID signing

With Developer ID, the binary's Designated Requirement is bound to
the Team ID, not the cdhash. Every Dayseam release signed by the
same Apple Developer account produces binaries that satisfy the
same DR, so a Keychain ACL granted to one release remains valid for
all future releases.

Pros: this is the *actual* fix. Prompts go from "once per release
per item" to **once per install, forever** — the UX bar the user
asked for. Consolidation (option B) would still be optional but no
longer necessary.

Cons: requires an Apple Developer Program subscription (\$99/yr),
identity verification paperwork, and wiring up
[`codesign -s "Developer ID Application: …"`](https://developer.apple.com/documentation/security/code_signing_services)
plus notarisation in the release workflow. Not landable in a patch
release.

### D. Do nothing, educate the user

Keep the current behaviour; explain in onboarding copy that macOS
will prompt per connector after each release and to click "Always
Allow."

Pros: zero code changes.

Cons: ships a UX wart as a permanent feature. Users
legitimately find repeated prompts alarming.

## Decision

**For v0.6.3: accept the current behaviour and defer to Developer
ID signing.** Rationale:

- The user explicitly weighed options A, B, C, D above (see DAY-121
  brainstorm) and chose C.
- Option A (removing the audit) shifts the prompts around without
  reducing their count, so it doesn't meet the "once on first
  install" bar.
- Option B (consolidation) is a one-time reduction of prompt
  cardinality that still doesn't meet the bar under ad-hoc signing.
  Its migration risk and concurrency cost are not justified given
  that Developer ID makes it unnecessary.
- Option D is the status quo.
- Option C is the only path that produces the UX the user actually
  asked for. Scoping it as its own ticket lets us plan the
  subscription, identity verification, and release-workflow
  changes without blocking the label-rename fixes in v0.6.3.

## Plan

1. **v0.6.3 (this release):** ship the DAY-121 label-rename UX fixes.
   Leave the keychain code and the orphan-secret audit as-is.
2. **v0.7.x (follow-up ticket):** pursue Apple Developer ID signing
   in a standalone ticket. Scope covers:
   - Enrolment in Apple Developer Program, identity verification.
   - CI secret management for the signing identity (`.p12` +
     keychain unlock on GitHub Actions runners, or migration to a
     macOS runner that already has the identity installed).
   - `scripts/release/build-dmg.sh` switch from `codesign -s -`
     (ad-hoc) to `codesign -s "Developer ID Application: <Team>"`.
   - Notarisation via `xcrun notarytool` and stapling via
     `xcrun stapler staple`.
   - `release.yml` wiring + smoke-test on a non-default release
     branch before flipping the default.
3. **No v0.7.x consolidation work is planned.** Once Developer ID
   lands, the three-keychain-items layout is fine — it produces
   three prompts on first install, and those grants persist across
   releases. Only revisit if user feedback after Developer ID
   still flags "too many prompts on first install".

## Re-evaluation triggers

- Developer ID enrolment blocked (identity verification rejection,
  team-account constraints, etc.) — if the follow-up ticket stalls
  for > 1 month, consider option B as a stop-gap.
- Surge in user reports that the three first-install prompts
  (post Developer ID) are themselves a UX problem.
- Any move away from `keyring-rs`/Keychain-as-store that would
  change the "per item prompts per release" arithmetic above.
