/**
 * Parity guard: the CLI's built-in extension list is not exported, so the
 * distro replicates it. This test reads the installed CLI's own registry and
 * fails when an upgrade adds, removes or reorders a built-in — the point is
 * that the next pi bump cannot silently leave one behind.
 *
 * `llama.cpp` is the known, deliberate gap: its factory is not exported from
 * the package root, so the drift test names it instead of hiding it.
 */

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { KNOWN_UNEXPORTED_BUILTINS, OFFICIAL_BUILTIN_EXTENSIONS, OFFICIAL_BUILTIN_NAMES } from "./official";

const PKG = join(import.meta.dir, "..", "..", "node_modules", "@earendil-works", "pi-coding-agent");
const REGISTRY = join(PKG, "dist", "extensions", "index.js");

describe("official built-in parity", () => {
  test("the CLI's built-in list is what this distro thinks it is", () => {
    const source = readFileSync(REGISTRY, "utf8");
    const names = [...source.matchAll(/name:\s*"([^"]+)"/g)].map((match) => match[1]!);
    expect(names).toEqual(["llama.cpp", "codemode", "tool-search", "mcp"]);
    const reproducible = names.filter((name) => !KNOWN_UNEXPORTED_BUILTINS.includes(name as never));
    expect([...OFFICIAL_BUILTIN_NAMES]).toEqual(reproducible);
  });

  test("replicated entries use the CLI's builtin/replaceable semantics", () => {
    expect(OFFICIAL_BUILTIN_EXTENSIONS.map((entry) => (typeof entry === "function" ? "inline" : entry.name))).toEqual([
      "codemode",
      "tool-search",
      "mcp",
    ]);
    for (const entry of OFFICIAL_BUILTIN_EXTENSIONS) {
      if (typeof entry === "function") throw new Error("expected a named builtin entry");
      expect(entry.builtin).toBe(true);
      expect(entry.replaceable).toBe(true);
      expect(typeof entry.factory).toBe("function");
    }
  });
});
