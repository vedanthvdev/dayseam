# Privacy & security

This document explains what Dayseam stores, where it stores it, what
leaves your machine, and what the macOS build is and isn't protected
against. If a question you have isn't answered here, please
[open an issue](https://github.com/dayseam/dayseam/issues/new)
tagged `documentation` and we'll fold the answer back in.

Dayseam is a local-first desktop app. The short version: your work
data stays on your machine, your tokens live in the macOS Keychain,
and the app talks to only three classes of endpoint — the source
hosts you configure, Apple's notarization check, and GitHub Releases
for updates. There is no Dayseam server, no analytics, and no
mandatory account.

## What Dayseam stores on your machine

Dayseam keeps one SQLite database per user profile in the macOS app
data directory (`~/Library/Application Support/dev.dayseam.desktop/`).
The tables are defined by the migrations in
[`crates/dayseam-db/migrations/`](../crates/dayseam-db/migrations/)
and cover, in plain terms:

- **`sources`** — the GitLab/GitHub/Jira/Confluence/Local-Git
  connections you've configured, including their base URLs, labels,
  and a reference to the Keychain entry that holds the token (the
  token value itself is **not** in this table).
- **`identities`** and **`source_identities`** / **`persons`** —
  the "this GitLab email is also this GitHub login" mappings used to
  attribute events to you across sources.
- **`local_repos`** — directories you've added as Local Git scan
  roots, plus a per-repo "redact in reports" flag for private repos.
- **`activity_events`** — the normalised, deduplicated stream of
  commits, merge/pull requests, issues, comments, and Confluence
  edits that feed reports.
- **`raw_payloads`** — the original upstream JSON (or Git object
  data) the activity events were derived from. This is what makes
  evidence links clickable back to concrete source material, and
  what lets the app re-derive activity without re-fetching.
- **`report_drafts`** — the reports you've generated, in their
  editable Markdown form.
- **`artifacts`** / **`sync_runs`** — sink outputs (e.g. saved
  Markdown files) and sync run metadata.
- **`log_entries`** — in-app log drawer contents for recent runs,
  including redacted error messages and per-source durations.
- **`sinks`** and **`settings`** — your output destinations
  (Markdown file path, Obsidian vault, etc.) and user preferences.

Because `raw_payloads` stores the verbatim upstream JSON, any fields
those upstream systems exposed to you (ticket titles, comment text,
commit messages, PR descriptions, etc.) are cached locally. For
private repos you've marked as such, the Local Git connector
suppresses content in rendered reports; the raw cache still exists
on disk.

No work data is uploaded anywhere by Dayseam. Backing up your home
directory backs up this database.

## Where tokens live

Personal access tokens, API tokens, and app-password style secrets
**do not live in SQLite**. They live in the macOS login Keychain,
accessed through [`crates/dayseam-secrets`](../crates/dayseam-secrets/)
using the `keyring` crate. Each secret is addressed by a
`service::account` pair (see
[`crates/dayseam-secrets/src/keychain.rs`](../crates/dayseam-secrets/src/keychain.rs))
so you can audit Dayseam's entries in **Keychain Access.app** under
the Dayseam service name.

When you paste a token into the Add/Edit Source dialog, the token
crosses the WebView → Rust IPC boundary exactly once on the way to
the Keychain. That crossing is handled by
[`IpcSecretString`](../apps/desktop/src-tauri/src/ipc/secret.rs),
which is a deliberately asymmetric wrapper:

- It implements `serde::Deserialize` so the WebView can hand a token
  to Rust.
- It does **not** implement `serde::Serialize`, so Rust cannot
  accidentally hand a token back to the WebView.
- Its `Debug` implementation is redacted.
- It zeroizes its inner memory on drop.

Once the token is in the Keychain, Rust fetches it on demand for
outbound HTTP calls and never re-sends it to the frontend. A rotated
or revoked token is replaced in place via the same IPC path.

## What leaves your machine

Dayseam initiates network traffic to three kinds of destination, and
only these three:

1. **The source hosts you configure.** For a GitLab source pointed
   at `gitlab.example.com`, Dayseam makes HTTPS requests to
   `gitlab.example.com`, authenticated with the token for that
   source. The same applies to GitHub (cloud or Enterprise Server
   at the base URL you provide), Jira Cloud, and Confluence Cloud.
   Local Git is entirely filesystem-based and makes no network
   calls. Self-hosted instances are a supported configuration;
   Dayseam will call whichever host you enter, including internal
   hosts on a corporate VPN, so choose the base URL with the same
   care you would give any other tool.
2. **GitHub Releases**, for the Tauri updater feed at
   `https://github.com/dayseam/dayseam/releases/latest/download/latest.json`
   and for downloading update artifacts when you approve an update.
   See [Updates and signature verification](#updates-and-signature-verification)
   below.
3. **Apple notarization services**, indirectly, on first launch of
   a signed build. This is a macOS-level check performed by
   Gatekeeper — not by Dayseam code — and is the same check every
   notarized Developer ID app triggers. Dayseam does not add any
   data to the check.

Everything else — reports, evidence, logs, identity mappings, raw
payloads — stays on the local disk.

## Telemetry

**Dayseam does not send telemetry.** There is no analytics SDK, no
crash reporter, no ping-home. The frontend has no third-party
script origins (the Content Security Policy is
`default-src 'self'; script-src 'self'`, see
[`apps/desktop/src-tauri/tauri.conf.json`](../apps/desktop/src-tauri/tauri.conf.json))
and the Rust workspace doesn't pull in analytics dependencies.

If you encounter a bug and we ask for diagnostic information, it
comes from the in-app log drawer and is something you explicitly
copy and share, not something Dayseam collects automatically.

## Updates and signature verification

The in-app updater is the Tauri updater plugin, configured in
[`apps/desktop/src-tauri/tauri.conf.json`](../apps/desktop/src-tauri/tauri.conf.json).

Before applying any update, the plugin verifies the update artifact
against an Ed25519 public key **baked into the app binary at build
time**. The corresponding private key is held by the maintainers;
an attacker who MITMs `latest.json` without also possessing that
private key cannot produce an artifact the updater will accept.

On top of that, official macOS builds are:

- Signed with a Developer ID Application certificate and a
  hardened runtime.
- Notarized by Apple (a second, independent trust root).
- Stapled, so Gatekeeper can verify the notarization ticket
  offline.

The full signing and notarization setup — including how to rotate
the cert or the notarytool password, and how to verify a specific
release artifact after the fact — is documented in
[`docs/release/CODESIGN.md`](./release/CODESIGN.md). Contributor
builds from a fork without Developer ID secrets fall back to an
ad-hoc signed DMG; the first-run Gatekeeper path for that case is
in [`docs/release/UNSIGNED-FIRST-RUN.md`](./release/UNSIGNED-FIRST-RUN.md).

The updater's Tauri capability grants are intentionally narrow
([`apps/desktop/src-tauri/capabilities/updater.json`](../apps/desktop/src-tauri/capabilities/updater.json)):
`updater:allow-check`, `updater:allow-download-and-install`, and
`process:allow-restart`. The split `allow-download` / `allow-install`
pair and `process:allow-exit` are deliberately **not** granted so
the WebView cannot pause a download mid-flight or force-quit the
app.

## macOS permissions the build asks for

Dayseam is a **direct-download, non-sandboxed** macOS app. It does
not go through the Mac App Store (that path is tracked as a future
feasibility spike, not a current distribution route). The
entitlements it requests, defined in
[`apps/desktop/src-tauri/entitlements.plist`](../apps/desktop/src-tauri/entitlements.plist),
are:

| Entitlement | Why |
|---|---|
| `com.apple.security.files.user-selected.read-write` | So you can pick scan roots and sink destinations via the macOS file picker. |
| `com.apple.security.cs.allow-jit` | Required by the WebView stack under hardened runtime. |
| `com.apple.security.cs.allow-unsigned-executable-memory` | Required by the WebView stack under hardened runtime. |

Dayseam does **not** request camera, microphone, contacts,
calendar, photos, location, full-disk, or accessibility permissions.
The only macOS permission prompt a user typically sees is the
Keychain-access prompt when a token is read for the first time by
a new app build, which is macOS's standard Keychain posture.

## What is and isn't protected

Dayseam protects against, at a minimum:

- Tokens sitting at rest in a file on disk — they're in the
  Keychain instead.
- A compromised renderer exfiltrating tokens directly — tokens are
  one-way across IPC and never re-sent to the frontend.
- A man-in-the-middle substituting updates — the Tauri updater
  verifies an Ed25519 signature and macOS verifies the Developer ID
  signature and notarization ticket on top.
- Accidental telemetry — there is no analytics code to accidentally
  expose something.

Dayseam does **not** protect against:

- **Local disk compromise without FileVault.** A cached
  `raw_payloads` row is a file on your disk. If your disk is
  readable by an attacker (stolen unlocked laptop, Time Machine
  disk without encryption, sync folder exposed to another account),
  they can read your cached upstream content. Turn on FileVault.
- **Malware running as your user.** Dayseam has the same file-
  system and Keychain access you do. A separate malicious process
  running under your user account, with your Keychain unlocked,
  can ask the Keychain for Dayseam's entries the same way Dayseam
  does. Keep your machine clean and your macOS login password
  strong.
- **A compromised token at the source.** If a GitLab PAT is
  stolen somewhere else entirely, the attacker has whatever
  Dayseam's copy of that token would have. Rotate tokens in the
  source UI and update them in Dayseam; the app also supports
  short-lived tokens.
- **User-chosen hosts being hostile.** Dayseam will call whatever
  base URL you give it for a self-hosted GitLab or GitHub
  Enterprise instance. That is correct for the product but
  worth being aware of in enterprise contexts.
- **Future WebView exploits.** The CSP is strict and the
  privileged Tauri surface is explicitly allow-listed (see
  [`apps/desktop/src-tauri/capabilities/default.json`](../apps/desktop/src-tauri/capabilities/default.json)
  and `updater.json`), but WebView sandboxing is a real defence-
  in-depth layer rather than a perfect boundary. Keep the app
  updated so underlying WebView fixes ship.

## Reporting security issues

For anything that looks like a security vulnerability, please open
a **private** security advisory via the repo's
[Security tab](https://github.com/dayseam/dayseam/security/advisories/new)
instead of a public issue. For everything else — including
questions about the model described here — regular GitHub issues
are the right channel.
