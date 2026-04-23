# `entitlements.plist` — rationale

**Do not merge this prose back into `entitlements.plist`.** macOS's
`codesign` invokes Apple's kernel-side `AMFIUnserializeXML` parser to
validate entitlements, and that parser does **not** accept XML
comments (`<!-- … -->`) — it is stricter than `plutil` or
`CoreFoundation`'s plist reader, even though both of those happily
round-trip commented files. Shipping the commented plist with v0.6.2
broke the release workflow at the `codesign` step inside
`cargo tauri build --bundles dmg,updater`:

```
Failed to parse entitlements: AMFIUnserializeXML: syntax error near line 30
failed to bundle project failed codesign application:
    failed to run command codesign: failed to sign app
```

DAY-120 fixes that by stripping every comment out of `entitlements.plist`
and moving the explanatory prose into this sibling markdown file. If
a future change needs to add an entitlement, edit the `.plist` as a
bare dict, document the reasoning here, and re-run
`scripts/release/build-dmg.sh <version>` locally to confirm codesign
still succeeds.

## Why the file exists

v0.6.1 shipped completely unsigned. On macOS 13+ an unsigned app that
calls `NSOpenPanel` (the native folder picker behind Tauri's
`dialog.open`) gets a *scoped* grant that is not persisted across
launches. Worse, every child directory the bundle reads inside a
TCC-protected root (`~/Desktop`, `~/Documents`, `~/Downloads`,
`~/Public`) can trigger a *separate* consent prompt, which is the
"5 or 6 pop-ups" cascade users reported when picking a scan root for
the local-git connector. The connector's protected-name skip list
(see `crates/connectors/connector-local-git/src/discovery.rs`) covers
part of that, but without an entitlements grant the OS still re-asks
on every fresh launch.

Adding `com.apple.security.files.user-selected.read-write` tells TCC
that when the user picks a folder via the system picker, the
resulting grant should persist for the life of the bundle. For a
hardened-runtime + ad-hoc-signed binary this is the widest the grant
can be without a Developer ID identity — which we do not have yet
(see the Phase 3.5 codesign tracking issue referenced in
`.github/workflows/release.yml`).

## What each key does

| Key | Why it's set |
|---|---|
| `com.apple.security.files.user-selected.read-write` | Persists grants the user gives through the native folder/file picker (`NSOpenPanel`) so that repeated `dialog.open()` calls and fresh launches after a scan root has been approved do not re-prompt. Directly addresses the v0.6.1 popup cascade. |
| `com.apple.security.cs.allow-unsigned-executable-memory` | Tauri 2's bundle links code that the hardened runtime would otherwise reject for writing executable pages. Matches the entitlements Tauri ships in its own macOS examples. |
| `com.apple.security.cs.allow-jit` | The webview is JIT-less but this is the standard hardened-runtime allowance Apple recommends for Electron/Tauri-style apps to cover any embedded JS engine path. |

## Known limitation / follow-up

Ad-hoc signing uses a cdhash-based Designated Requirement, so a later
release will have a different cdhash from v0.6.2 and the Keychain may
re-prompt once at upgrade time. A truly stable DR tied to a bundle id
+ Team ID requires a Developer ID signing identity and is tracked as
follow-up work (issues #59 / #108). For v0.6.2 this is still a
substantial improvement over v0.6.1 (which shipped completely
unsigned and therefore re-prompted on *every* launch, not just
upgrades).

## Why no `app-sandbox`

Sandbox entitlements are intentionally **not** set. The Dayseam
desktop shell needs to walk user-selected code trees, launch the
system keyring (macOS Keychain), and open URLs via the `opener`
crate; all three of those paths break under the App Sandbox unless
we add temporary exception entitlements we do not want to maintain.
The shell is local-first and ships with a strict CSP
(`default-src 'self'`) plus a narrow IPC capability list, so the
marginal isolation the sandbox adds is not worth the loss of
legitimate filesystem access.
