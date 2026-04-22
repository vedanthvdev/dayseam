// Parity tests for the Tauri IPC command catalogue.
//
// The production command surface is declared in *three* places that
// must stay in lockstep:
//
//   1. Rust: `apps/desktop/src-tauri/src/ipc/commands.rs::PROD_COMMANDS`
//      — also wired into `invoke_handler!` and `build.rs::COMMANDS`.
//   2. Capabilities: `capabilities/default.json` — Tauri 2 rejects any
//      command whose `allow-*` permit is missing.
//   3. TS: `@dayseam/ipc-types` — the `Commands` interface + the
//      `PROD_COMMANDS` runtime list used by callers of `invoke`.
//
// Drift between any two of those makes the app silently broken at
// runtime. The Rust-side parity is covered by the
// `capabilities::default_capability_covers_every_production_command`
// integration test; these Vitest cases cover the TS surface.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  DEV_COMMANDS,
  PROD_COMMANDS,
  type Commands,
} from "@dayseam/ipc-types";

const HERE = dirname(fileURLToPath(import.meta.url));
const CAPABILITY_PATH = resolve(
  HERE,
  "../../src-tauri/capabilities/default.json",
);

interface Capability {
  permissions: string[];
}

function loadCapability(): Capability {
  const raw = readFileSync(CAPABILITY_PATH, "utf8");
  return JSON.parse(raw) as Capability;
}

/** Tauri rewrites `_` to `-` when generating permissions. */
function allowPermit(command: string): string {
  return `allow-${command.replaceAll("_", "-")}`;
}

// Exhaustive object keyed by `keyof Commands`. If a new command is
// added to the `Commands` interface without updating
// `PROD_COMMANDS`, TypeScript will fail to compile this object
// because one of the keys will be missing.
const commandSurface: Record<keyof Commands, true> = {
  settings_get: true,
  settings_update: true,
  logs_tail: true,
  persons_get_self: true,
  persons_update_self: true,
  sources_list: true,
  sources_add: true,
  sources_update: true,
  sources_delete: true,
  sources_healthcheck: true,
  identities_list_for: true,
  identities_upsert: true,
  identities_delete: true,
  local_repos_list: true,
  local_repos_set_private: true,
  sinks_list: true,
  sinks_add: true,
  report_generate: true,
  report_cancel: true,
  report_get: true,
  report_save: true,
  retention_sweep_now: true,
  activity_events_get: true,
  shell_open: true,
  gitlab_validate_pat: true,
  atlassian_validate_credentials: true,
  atlassian_sources_add: true,
  atlassian_sources_reconnect: true,
  github_validate_credentials: true,
  github_sources_add: true,
  github_sources_reconnect: true,
  dev_emit_toast: true,
  dev_start_demo_run: true,
};

describe("IPC command catalogue parity", () => {
  it("PROD_COMMANDS + DEV_COMMANDS covers every key of the Commands interface", () => {
    const declared = new Set<keyof Commands>([
      ...PROD_COMMANDS,
      ...DEV_COMMANDS,
    ]);
    const typed = new Set<keyof Commands>(
      Object.keys(commandSurface) as (keyof Commands)[],
    );
    expect([...declared].sort()).toEqual([...typed].sort());
  });

  it("PROD_COMMANDS and DEV_COMMANDS don't overlap", () => {
    const dev = new Set<string>(DEV_COMMANDS);
    const leaks = PROD_COMMANDS.filter((c) => dev.has(c));
    expect(leaks).toEqual([]);
  });

  it("capabilities/default.json grants every production command", () => {
    const capability = loadCapability();
    const granted = new Set(capability.permissions);
    const missing = PROD_COMMANDS.filter((c) => !granted.has(allowPermit(c)));
    expect(missing).toEqual([]);
  });

  it("capabilities/default.json does not grant any unknown command", () => {
    const capability = loadCapability();
    const expected = new Set(PROD_COMMANDS.map(allowPermit));
    const stale = capability.permissions
      .filter((p) => p.startsWith("allow-"))
      .filter((p) => !expected.has(p));
    expect(stale).toEqual([]);
  });
});
