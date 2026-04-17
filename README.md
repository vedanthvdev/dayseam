# Dayseam

Open-source, local-first automated work reporting.

Dayseam connects to the tools you use every day — starting with local git
repositories and self-hosted GitLab — and stitches together an
evidence-linked report of what you actually did on a given day. Pick a
date, click generate, and get a clean markdown report you can save to
disk or drop into your Obsidian vault.

> **Status: early design, not yet shippable.** This repository currently
> contains only the licence, this README, and a `.gitignore`. The first
> working version (v0.1) will ship as a Mac app with local git and
> self-hosted GitLab sources, and a markdown/Obsidian sink.

## Why

Nobody loves writing end-of-day reports. Most of us juggle many tools
in parallel — multiple repos, multiple MRs in flight, issue threads,
code review — and reconstructing "what did I actually do today" from
memory is both slow and inaccurate. Dayseam does the reconstruction
for you, from the evidence your tools already keep.

## Design principles

- **Local-first.** Your data never leaves your machine unless you
  explicitly publish a report somewhere.
- **No mandatory central account.** Nothing to sign up for to use the
  app.
- **Read-only source connectors by default.** Write access is explicit,
  per destination.
- **Draft-first.** Nothing is auto-sent without your review.
- **Every generated line must be explainable by evidence.** Click any
  bullet and see which commits, merge requests, or comments fed into
  it.
- **Pluggable architecture.** New sources and sinks can be added
  without touching the core.
- **Never fail silently.** Every collector streams progress; every
  error carries a clear message and a suggested action.

## Roadmap

- **v0.1 (in design).** Mac desktop app (Tauri). Local git + self-hosted
  GitLab sources. Markdown + Obsidian sink. Single-day reports.
- **v0.2+.** OAuth for GitLab, date-range reports, scheduler,
  additional connectors (GitHub, Jira, Slack, Confluence).
- **Later.** Optional local LLM rewrite, Windows/Linux builds, team
  features.

## Architecture (sketch)

A TypeScript/React frontend inside a Tauri shell, with a Rust core that
owns connectors, the SQLite activity store, the report engine, and the
sink adapters. Every connector and sink implements a small, typed
interface, so adding a new source (say, Jira) or a new destination
(say, Notion) is a standalone crate that doesn't touch the core.

Full design document will be committed to
`docs/design/v0.1-design.md` once the design phase completes.

## Contributing

Contributions welcome once v0.1 lands. In the meantime, feel free to
open issues with ideas or questions.

## Licence

[AGPL-3.0](./LICENSE). If you want to use Dayseam inside a commercial
product in a way AGPL doesn't allow, contact the maintainer about a
commercial licence.
