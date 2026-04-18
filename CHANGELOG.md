# Changelog

All notable changes to Dayseam are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial monorepo scaffold: Cargo workspace with seven crate skeletons, pnpm
  workspace with a Tauri + React + TypeScript + Tailwind desktop app shell,
  CI pipeline (rust, frontend, check-semver-label), PR template, and branch
  protection setup script.
- `dayseam-core` domain types, `DayseamError` taxonomy with stable error
  codes, and ts-rs-generated TypeScript bindings committed to
  `packages/ipc-types/src/generated/`.
- `dayseam-db`: SQLite persistence layer with the v1 schema from design
  §5.2, a `sqlx`-managed migration, and typed repositories for every table
  (`SourceRepo`, `IdentityRepo`, `LocalRepoRepo`, `ActivityRepo`,
  `RawPayloadRepo`, `DraftRepo`, `LogRepo`, `SettingsRepo`). `open(path)`
  enables WAL + foreign keys and is idempotent across re-opens.
- `dayseam-secrets`: `Secret<T>` wrapper with redacting `Debug`/`Display`
  and zeroing `Drop`, a narrow `SecretStore` trait, an `InMemoryStore`
  for tests, and a feature-gated `KeychainStore` that stores tokens in
  the macOS Keychain under a `service::account` composite key. Delete is
  idempotent and the macOS round-trip is covered by an `#[ignore]`d
  smoke test.
