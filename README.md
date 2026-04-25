# Dayseam

Open-source, local-first automated work reporting.

Dayseam connects to the tools you use every day — local Git
repositories, GitLab (cloud or self-hosted), GitHub (cloud or Enterprise),
Jira, and Confluence — and stitches them into an evidence-linked report
of what you actually did on a given day. Pick a date, click generate,
and get a clean Markdown report you can save to disk or drop into your
Obsidian vault.

> **Status: public beta.** Current release is `v0.8.2`. The macOS
> desktop app ships five source connectors (Local Git, GitLab, GitHub,
> Jira, Confluence), a Markdown/Obsidian sink, cross-source dedup,
> evidence-linked bullets, an in-app scheduler, and a Tauri updater
> over signed + notarized DMGs. Windows and Linux builds are on the
> post-v1.0 roadmap.

## Install

macOS 13 (Ventura) or newer. Download the latest DMG from the
[Releases page](https://github.com/dayseam/dayseam/releases/latest)
(universal binary — Apple Silicon and Intel, same file).

Official releases are signed with a Developer ID Application
certificate, notarized by Apple, and stapled, so a normal double-click
install works without a Gatekeeper prompt. If the repo is ever
temporarily built without Developer ID secrets, the resulting DMG is
ad-hoc signed instead — the unsigned-first-run path is documented in
[`docs/release/UNSIGNED-FIRST-RUN.md`](./docs/release/UNSIGNED-FIRST-RUN.md)
for that case and for contributors building from a fork. The signing
model, rotation, and verification are documented in full at
[`docs/release/CODESIGN.md`](./docs/release/CODESIGN.md).

Once installed, the built-in Tauri updater checks the GitHub Releases
feed, verifies an Ed25519 signature on the update payload, and applies
updates in the background. See [Privacy & security](#privacy--security)
for the update trust model.

## Why

Nobody loves writing end-of-day reports. Most of us juggle many tools
in parallel — multiple repos, multiple MRs in flight, issue threads,
code review, linked Confluence pages — and reconstructing "what did I
actually do today" from memory is both slow and inaccurate. Dayseam
does the reconstruction for you, from the evidence your tools already
keep.

## Design principles

- **Local-first.** Your data never leaves your machine unless you
  explicitly publish a report somewhere.
- **No mandatory central account.** Nothing to sign up for to use the
  app.
- **Read-only source connectors by default.** Write access is explicit,
  per destination.
- **Draft-first.** Nothing is auto-sent without your review.
- **Every generated line must be explainable by evidence.** Click any
  bullet and see which commits, merge requests, issues, or comments
  fed into it.
- **Connector architecture.** Sources and sinks are compile-time
  extensions that each implement a small, typed trait
  (`SourceConnector` / `SinkAdapter`). Adding a new source or
  destination is a standalone crate that doesn't touch the core —
  no runtime plugin loading, no dynamic code, no extra signing surface.
- **Never fail silently.** Every collector streams progress; every
  error carries a clear message and a suggested action.

## What Dayseam connects to

| Source | Hosting | Auth | Notes |
|---|---|---|---|
| Local Git | Any directory you point it at | None (filesystem) | Private repos can be marked so their content is redacted from reports. |
| GitLab | gitlab.com or any self-hosted instance | Personal access token | Token scopes documented in the in-app setup flow. |
| GitHub | github.com or GitHub Enterprise Server | Personal access token | Custom base URL supported for Enterprise. |
| Jira | Atlassian Cloud (self-hosted deferred) | API token + email | Read-only; pulls ticket titles and state for key enrichment. |
| Confluence | Atlassian Cloud (self-hosted deferred) | Shared with Jira | Docs/comments treated as supporting evidence, not automation targets. |

Output sinks today: Markdown file (including the Obsidian-flavoured
YAML frontmatter). More sinks are planned, but any new sink is an
explicit, user-chosen write destination.

## Privacy & security

Dayseam is built around keeping your work data on your machine:

- Long-lived tokens live in the macOS Keychain, not in the SQLite
  database.
- There is no Dayseam server. No analytics. No telemetry. The only
  network calls are to the source hosts you configure and to GitHub
  Releases for updates.
- Raw upstream payloads, report drafts, logs, and source metadata
  are cached locally in SQLite so the app can show evidence without
  re-fetching. That data is covered by your normal disk and backup
  posture.

Full details — including the update trust model, what leaves the
machine, and what isn't protected against — are in
[`docs/privacy-security.md`](./docs/privacy-security.md).

## Architecture & roadmap

The canonical, living reference is [`ARCHITECTURE.md`](./ARCHITECTURE.md).
It covers the system top-to-bottom — principles, repo layout, backend
and frontend architecture, the connector and sink contracts, and the
versioned roadmap through v1.0.

Short version:

- **Shape.** TypeScript/React frontend inside a Tauri 2 shell, with
  a Rust core that owns connectors, the SQLite activity store, the
  report engine, and the sink adapters.
- **Shipping today (v0.8.x).** Five sources (Local Git, GitLab,
  GitHub, Jira, Confluence), Markdown/Obsidian sink, cross-source
  dedup, evidence-linked bullets, in-app scheduler, signed +
  notarized macOS DMG, Tauri updater.
- **Next (v0.9 → v1.0).** Stability, documentation, verification of
  a Developer ID artifact end-to-end, additional polish and UX
  work from the public-beta review pass.
- **Post-v1.0.** Cross-platform builds (Windows, Linux), additional
  connectors and sinks, optional AI rewrite.

## Contributing

Contributions are welcome. See [`AGENTS.md`](./AGENTS.md) for the
non-negotiable rules that both human contributors and AI coding
agents are expected to follow in this repo (no direct pushes to
`master`, no agent-driven merges, semver label conventions, and the
one-commit-per-branch workflow). Open an issue first for anything
non-trivial so scope can be agreed before you start.

## Licence

[AGPL-3.0](./LICENSE). If you want to use Dayseam inside a commercial
product in a way AGPL doesn't allow, contact the maintainer about a
commercial licence.
