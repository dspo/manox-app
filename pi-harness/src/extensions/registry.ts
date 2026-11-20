/**
 * The registry of the extensions Zu ships inside the app.
 *
 * Until now a built-in extension was an anonymous factory in
 * `PiHarnessSession.build`: no name, no switch, no skills. The consumers
 * that follow (ask, todos, visualize, artifacts) each need an identity so
 * that:
 *
 * - `disabledExtensions` in `~/.steer/settings.json` can turn one off
 *   without touching the always-on gate/dispatch pair;
 * - a skill belongs to the extension that uses it: disable the extension,
 *   and its skill stops loading too (`skillDirs` → `additionalSkillPaths`);
 * - a name conflict has an owner in pi's diagnostics (`zu:<name>`).
 *
 * This file is the mechanism only; the static `ZU_EXTENSIONS` ships empty on
 * purpose. The real entries (A1's ask, A2's todos) need per-session state, so
 * they compose at build time in `PiHarnessSession.build` — one entry, one
 * skill dir, no changes to the wiring.
 *
 * ## Precedence (verified against pi 1.5.0, not assumed)
 *
 * pi resolves a tool name by extension load order: the first registration
 * wins (`ExtensionRunner.getAllRegisteredTools` keeps the first per name).
 * Two things follow, and they are why `pi/services.ts` reorders:
 *
 * 1. among the host's inline factories, the array order is the precedence —
 *    earlier wins;
 * 2. the resource loader appends *every* inline factory after the extension
 *    files it found in `~/.pi` and the project, so an unmodified loader lets
 *    a user package shadow a host tool (probe: a package's `probeTool` beat
 *    an inline factory's, while the same two inline factories resolved to
 *    the earlier one).
 *
 * The host binds some tool names (dispatch, and later the A1–A4 tools) to UI
 * surfaces, so `createPiServices` moves inline (host) extensions ahead of
 * file extensions before the session binds. Registry entries are named
 * `zu:<name>` so that decision and its conflict diagnostics stay readable.
 */

import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI, InlineExtension } from "@earendil-works/pi-coding-agent";

/** The Settings key: also the `disabledExtensions` entry. Stable forever. */
export interface ZuExtension {
  /** One bare name, used as the settings key and the skill directory name. */
  name: string;
  /** Built with the session's settings, so a reload re-decides. */
  factory: (settings: ZuExtensionSettings) => (pi: ExtensionAPI) => void;
  /** Absolute skill roots owned by this extension. Omitted = no skills. */
  skillDirs?: string[];
  /**
   * Off unless the user enables it. Defaults to true. v1 reads only
   * `disabledExtensions`, so a `false` here currently means "never enabled"
   * — the opt-in list is a later issue, not a missing branch.
   */
  defaultEnabled?: boolean;
}

/** What the harness reads from `~/.steer/settings.json`, once per session. */
export interface ZuExtensionSettings {
  /** The extensions the user turned off. */
  disabled: ReadonlySet<string>;
  /** The whole file, for an extension's own future settings keys. */
  raw: Record<string, unknown>;
}

/** What `composeExtensions` hands to `createPiServices`. */
export interface ComposedExtensions {
  /** Named inline factories (`<inline:zu:name>`), in registry order. */
  factories: InlineExtension[];
  /** Skill roots of the enabled extensions, deduped, registry order. */
  skillDirs: string[];
}

/** The inline-factory name prefix: `zu:todos` shows as `<inline:zu:todos>`. */
export const ZU_EXTENSION_PREFIX = "zu:";

/**
 * The extensions this build ships as static entries. Empty on purpose: the
 * mechanism is tested with fixtures, and every real entry so far needs
 * per-session state, so it composes at build time instead — A1's ask (the
 * mailbox bridge) and A2's todos (the store keyed by the session id) are
 * appended in `PiHarnessSession.build` as `todosExtensionEntry(...)`-style
 * helpers, and this array stays free for genuinely static entries.
 */
export const ZU_EXTENSIONS: ZuExtension[] = [];

/**
 * Where the bundled skills live, in both layouts:
 *
 * - compiled: `Contents/Helpers/pi-acp` → `Contents/Helpers/skills`;
 * - source run: `pi-harness/src/extensions/registry.ts` → `pi-harness/skills`.
 *
 * `bun run build` (`scripts/build.ts`) copies the same tree beside the
 * executable and asserts every `SKILL.md` made it, so the compiled branch is
 * reachable.
 */
export function skillsBase(): string {
  const beside = join(dirname(process.execPath), "skills");
  if (existsSync(beside)) return beside;
  return join(dirname(fileURLToPath(import.meta.url)), "..", "..", "skills");
}

/** The skill root an extension owns: `skills/<name>` in both layouts. */
export function bundledSkillDir(name: string): string {
  return join(skillsBase(), name);
}

/**
 * Build the enabled extensions and collect their skill roots.
 *
 * - disabled when the user listed the name, or when `defaultEnabled` is false
 *   (v1 has no opt-in list, so off stays off);
 * - a duplicate `name` throws: the registry is a compile-time constant, so a
 *   duplicate is a bug, not user input;
 * - each factory is built exactly once per call; a reload composes again, so
 *   settings changes take effect on the next `session/new` or `session/load`;
 * - `skillDirs` keeps registry order and drops repeats (two extensions may
 *   share a skill root, and the same path twice would only add diagnostics).
 */
export function composeExtensions(
  registry: readonly ZuExtension[],
  settings: ZuExtensionSettings,
): ComposedExtensions {
  const factories: InlineExtension[] = [];
  const skillDirs: string[] = [];
  const seen = new Set<string>();
  for (const extension of registry) {
    if (seen.has(extension.name)) {
      throw new Error(`duplicate Zu extension name "${extension.name}"`);
    }
    seen.add(extension.name);
    if (extension.defaultEnabled === false) continue;
    if (settings.disabled.has(extension.name)) continue;
    factories.push({
      name: `${ZU_EXTENSION_PREFIX}${extension.name}`,
      factory: extension.factory(settings),
    });
    for (const dir of extension.skillDirs ?? []) {
      if (!skillDirs.includes(dir)) skillDirs.push(dir);
    }
  }
  return { factories, skillDirs };
}
