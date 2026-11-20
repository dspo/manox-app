/**
 * The app's `settings.json`, read-only from the harness side.
 *
 * The app (`app/domains/state.ts` over `zu/state`) is the only writer, at
 * `~/.steer/settings.json`; the harness only needs the extension switches.
 * Reading is deliberately forgiving: a missing, half-written or corrupt file
 * (the app writes it atomically, but a hand edit is fair game) must never
 * stop a session from starting — it means "no extension is disabled".
 *
 * The same state root already backs the subagent store
 * (`zuStateDir(agentDir)/subagents`), hence the shared derivation.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type { ZuExtensionSettings } from "./extensions/registry";

/** The app's settings file name inside the state directory. */
export const ZU_SETTINGS_FILE = "settings.json";

/**
 * The app's state root, normally `~/.steer`. The app injects `ZU_STATE_DIR`
 * when it spawns the harness; the agent-dir derivation is the fallback for
 * standalone runs and older callers.
 */
export function zuStateDir(agentDir: string = getAgentDir()): string {
  const explicit = process.env.ZU_STATE_DIR?.trim();
  return explicit ? explicit : join(agentDir, "..", "zu");
}

/**
 * Read the extension switches once per session build.
 *
 * Only `disabledExtensions` is interpreted: an array whose string entries are
 * names. Anything else (missing file, bad JSON, a non-array field, non-string
 * entries) is ignored rather than thrown — unknown keys are kept in `raw` for
 * extensions that will read their own settings.
 */
export async function readZuSettings(agentDir: string = getAgentDir()): Promise<ZuExtensionSettings> {
  const raw = readRawSettings(join(zuStateDir(agentDir), ZU_SETTINGS_FILE));
  const disabled = new Set<string>();
  const listed = raw["disabledExtensions"];
  if (Array.isArray(listed)) {
    for (const name of listed) {
      if (typeof name === "string" && name.trim().length > 0) disabled.add(name.trim());
    }
  }
  return { disabled, raw };
}

/** The file as an object, or `{}` when it cannot be one. */
function readRawSettings(path: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return {};
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return {};
  return parsed as Record<string, unknown>;
}
