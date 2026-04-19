// Guard test: `dev_start_demo_run` and `dev_emit_toast` are feature-
// gated on the Rust side via `cfg(feature = "dev-commands")` so they
// compile out of release builds. If a UI file imports them by name
// (invoke("dev_start_demo_run", ...)), every release build still
// ships the string reference, and the command would be missing at
// runtime — the user would see a `No handler registered` error
// instead of the command's intended behaviour.
//
// This test walks the production `src/` tree (everything under
// `apps/desktop/src/` *except* `__tests__/`) and fails if any file
// mentions a dev-only command literal. Tests and the IPC types
// package itself are allowed to reference the names for their own
// runtime checks.

import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const FORBIDDEN = ["dev_start_demo_run", "dev_emit_toast"] as const;

// `process.cwd()` is `apps/desktop` when Vitest runs from the
// package's `test` script. Hard-coding the suffix keeps the test
// independent of how `import.meta.url` is shaped in the current
// Vite runtime (which has varied across releases).
const ROOT = resolve(process.cwd(), "src");
const EXCLUDED_DIRS = new Set(["__tests__"]);
const EXCLUDED_FILES = new Set<string>([]);

function walk(dir: string, out: string[]): void {
  for (const entry of readdirSync(dir)) {
    const abs = join(dir, entry);
    const stats = statSync(abs);
    if (stats.isDirectory()) {
      if (EXCLUDED_DIRS.has(entry)) continue;
      walk(abs, out);
      continue;
    }
    if (!/\.(ts|tsx)$/.test(entry)) continue;
    if (EXCLUDED_FILES.has(entry)) continue;
    out.push(abs);
  }
}

describe("dev command references in production src/", () => {
  it("no file under apps/desktop/src/ (excluding __tests__) mentions dev-only IPC commands", () => {
    const files: string[] = [];
    walk(ROOT, files);
    const offenders: { file: string; command: string }[] = [];
    for (const file of files) {
      const body = readFileSync(file, "utf8");
      for (const name of FORBIDDEN) {
        if (body.includes(name)) {
          offenders.push({ file: relative(ROOT, file), command: name });
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
