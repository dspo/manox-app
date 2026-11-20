/**
 * The official CLI's built-in extension set, rebuilt from public SDK factories.
 *
 * `pi`'s CLI prepends `llama.cpp`, `codemode`, `tool-search` and `mcp` to the
 * resource loader's factories (`dist/extensions/index.js`). SDK embeddings do
 * not get that list for free. This module is the parity point: a drift test
 * reads the installed CLI's list and fails when it changes, so an upgrade
 * cannot silently leave a built-in behind.
 *
 * `llama.cpp` is the one known gap: its factory is not exported from the
 * package root, so an SDK host has no public way to register it. The drift
 * test names it explicitly instead of pretending the list is complete.
 */

import {
  createCodemodeExtension,
  createMcpExtension,
  createToolSearchExtension,
  type InlineExtension,
} from "@earendil-works/pi-coding-agent";

/** The built-ins this distro can reproduce with exported factories. */
export const OFFICIAL_BUILTIN_NAMES = ["codemode", "tool-search", "mcp"] as const;

/** Built-ins the CLI has but the public API cannot supply yet. */
export const KNOWN_UNEXPORTED_BUILTINS = ["llama.cpp"] as const;

/**
 * `builtin: true` is load-bearing: it gives the same `-builtin:<name>`,
 * `--no-extensions`, package-replacement and project-trust ordering as the
 * CLI. A plain inline factory would bypass all of that.
 */
export const OFFICIAL_BUILTIN_EXTENSIONS: InlineExtension[] = [
  {
    name: "codemode",
    factory: createCodemodeExtension(),
    replaceable: true,
    builtin: true,
  },
  {
    name: "tool-search",
    factory: createToolSearchExtension(),
    replaceable: true,
    builtin: true,
  },
  {
    name: "mcp",
    factory: createMcpExtension(),
    replaceable: true,
    builtin: true,
  },
];
