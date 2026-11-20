/**
 * The built-in extension registry: naming, the `disabledExtensions` switch,
 * and skill ownership. Pure functions except the last block, which proves the
 * switch reaches the real resource loader's skill discovery.
 */

import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createPiServices } from "../pi/services";
import {
  bundledSkillDir,
  composeExtensions,
  skillsBase,
  ZU_EXTENSIONS,
  type ZuExtension,
  type ZuExtensionSettings,
} from "./registry";

const roots: string[] = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function settings(disabled: string[] = [], raw: Record<string, unknown> = {}): ZuExtensionSettings {
  return { disabled: new Set(disabled), raw };
}

/** A registry entry with a no-op factory unless one is given. */
function extension(name: string, options: Partial<ZuExtension> = {}): ZuExtension {
  return { name, factory: () => () => {}, ...options };
}

describe("composeExtensions", () => {
  test("an empty registry composes to nothing", () => {
    expect(composeExtensions([], settings())).toEqual({ factories: [], skillDirs: [] });
  });

  test("the shipped registry is a short list of unique names", () => {
    const names = ZU_EXTENSIONS.map((entry) => entry.name);
    expect(new Set(names).size).toBe(names.length);
  });

  test("enabled entries become named inline factories in registry order", () => {
    const first = extension("ask");
    const second = extension("todos");
    const composed = composeExtensions([first, second], settings());
    expect(composed.factories.map((factory) => (factory as { name: string }).name)).toEqual([
      "zu:ask",
      "zu:todos",
    ]);
    expect(composed.factories.every((factory) => typeof (factory as { factory: unknown }).factory === "function")).toBe(
      true,
    );
  });

  test("each factory is built once per composition and sees the settings", () => {
    const seen: ZuExtensionSettings[] = [];
    const entry = extension("ask", {
      factory: (given) => {
        seen.push(given);
        return () => {};
      },
    });
    const given = settings([], { sidebarWidth: 232 });
    const composed = composeExtensions([entry], given);
    expect(seen).toEqual([given]);
    expect(composed.factories).toHaveLength(1);
  });

  test("a disabled extension is skipped; the rest still compose", () => {
    const composed = composeExtensions([extension("ask"), extension("todos")], settings(["ask"]));
    expect(composed.factories.map((factory) => (factory as { name: string }).name)).toEqual(["zu:todos"]);
  });

  test("defaultEnabled=false stays off: v1 has no opt-in list", () => {
    const composed = composeExtensions([extension("artifacts", { defaultEnabled: false }), extension("todos")], settings());
    expect(composed.factories.map((factory) => (factory as { name: string }).name)).toEqual(["zu:todos"]);
  });

  test("a duplicate name throws, even when both copies are disabled", () => {
    expect(() => composeExtensions([extension("ask"), extension("ask")], settings())).toThrow(
      /duplicate Zu extension name "ask"/,
    );
    expect(() =>
      composeExtensions([extension("ask"), extension("ask")], settings(["ask", "ask"])),
    ).toThrow(/duplicate Zu extension name "ask"/);
  });

  test("skills come only from enabled extensions, deduped, in registry order", () => {
    const composed = composeExtensions(
      [
        extension("todos", { skillDirs: ["/skills/todos"] }),
        extension("artifacts", { skillDirs: ["/skills/artifacts"] }),
        extension("ask", { skillDirs: ["/skills/shared", "/skills/todos"] }),
      ],
      settings(["artifacts"]),
    );
    expect(composed.skillDirs).toEqual(["/skills/todos", "/skills/shared"]);
  });

  test("an extension with no skillDirs contributes no path", () => {
    expect(composeExtensions([extension("gate")], settings()).skillDirs).toEqual([]);
  });
});

describe("skillsBase", () => {
  test("the source layout resolves to pi-harness/skills, and the tree exists", () => {
    expect(skillsBase()).toBe(join(import.meta.dir, "..", "..", "skills"));
    expect(skillsBase().replaceAll("\\", "/").endsWith("/pi-harness/skills")).toBe(true);
  });

  test("an extension owns skills/<name> under the same base", () => {
    expect(bundledSkillDir("todos")).toBe(join(skillsBase(), "todos"));
  });
});

describe("skills follow the switch through the resource loader", () => {
  function sandbox(): { agentDir: string; cwd: string; skills: string } {
    const root = mkdtempSync(join(tmpdir(), "zu-registry-"));
    roots.push(root);
    const agentDir = join(root, "agent");
    const cwd = join(root, "project");
    const skills = join(root, "skills");
    mkdirSync(agentDir, { recursive: true });
    mkdirSync(cwd, { recursive: true });
    for (const [name, body] of [
      ["todos", "the todos skill"],
      ["ask", "the ask skill"],
    ] as const) {
      mkdirSync(join(skills, name), { recursive: true });
      writeFileSync(
        join(skills, name, "SKILL.md"),
        `---\nname: ${name}\ndescription: ${body}\n---\n\n${body}\n`,
      );
    }
    return { agentDir, cwd, skills };
  }

  test("an enabled extension's skill loads; a disabled one's does not", async () => {
    const { agentDir, cwd, skills } = sandbox();
    const registry = [
      extension("todos", { skillDirs: [join(skills, "todos")] }),
      extension("ask", { skillDirs: [join(skills, "ask")] }),
    ];

    // pi also discovers the developer's `~/.agents/skills`, so these assert
    // membership, never the whole list.
    const enabled = composeExtensions(registry, settings());
    const on = await createPiServices({ cwd, agentDir, trusted: true, additionalSkillPaths: enabled.skillDirs });
    const onNames = on.services.resourceLoader.getSkills().skills.map((skill) => skill.name);
    expect(onNames).toContain("todos");
    expect(onNames).toContain("ask");

    const off = composeExtensions(registry, settings(["todos"]));
    const only = await createPiServices({ cwd, agentDir, trusted: true, additionalSkillPaths: off.skillDirs });
    const offNames = only.services.resourceLoader.getSkills().skills.map((skill) => skill.name);
    expect(offNames).toContain("ask");
    expect(offNames).not.toContain("todos");
  });
});
