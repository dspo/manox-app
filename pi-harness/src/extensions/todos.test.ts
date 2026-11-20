/**
 * The `todos` extension entry (A2): the shipped skill loads through the real
 * resource loader, the six tools register and run against the store the
 * entry's context names, and the prompt section rides every
 * `before_agent_start` with byte-stable text. No live session needed — the
 * tools are driven the way pi would drive them.
 */

import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createPiServices } from "../pi/services";
import { bundledSkillDir } from "./registry";
import { todosExtensionEntry } from "./todos";
import { TODOS_RULE } from "../todos/tools";
import { readPlan } from "../todos/core";

const roots: string[] = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

interface FakePi {
  events: Map<string, Array<(...args: unknown[]) => unknown>>;
  tools: Map<string, { name: string; execute: (...args: unknown[]) => Promise<unknown> }>;
}

/** Records `on` handlers and `registerTool` entries the way pi binds them. */
function fakePi(): FakePi & ExtensionAPI {
  const pi: FakePi = { events: new Map(), tools: new Map() };
  const api = {
    on(event: string, handler: (...args: unknown[]) => unknown) {
      const list = pi.events.get(event) ?? [];
      list.push(handler);
      pi.events.set(event, list);
      return () => {};
    },
    registerTool(tool: { name: string; execute: (...args: unknown[]) => Promise<unknown> }) {
      pi.tools.set(tool.name, tool);
    },
  };
  return Object.assign(api, { events: pi.events, tools: pi.tools }) as FakePi & ExtensionAPI;
}

/** A sandbox agent dir; the store root derives to `<sandbox>/../zu/todos`. */
function sandboxAgentDir(): string {
  const root = mkdtempSync(join(tmpdir(), "zu-todos-ext-"));
  roots.push(root);
  const agentDir = join(root, "agent");
  mkdirSync(agentDir, { recursive: true });
  return agentDir;
}

describe("the todos entry", () => {
  test("the shipped skill exists under the bundled skills base", () => {
    const dir = bundledSkillDir("todos");
    expect(existsSync(join(dir, "SKILL.md"))).toBe(true);
    const body = readFileSync(join(dir, "SKILL.md"), "utf8");
    expect(body).toContain("name: todos");
    expect(body.startsWith("---\n")).toBe(true);
  });

  test("the entry owns the skill dir and builds a factory per composition", () => {
    const entry = todosExtensionEntry("sess-1", "/home/u/.pi/agent");
    expect(entry.name).toBe("todos");
    expect(entry.skillDirs).toEqual([join("/home/u/.pi/agent", "..", "zu", "..", "skills", "todos")].map(() => bundledSkillDir("todos")));
    expect(typeof entry.factory).toBe("function");
    expect(entry.factory({ disabled: new Set(), raw: {} })).toBeTypeOf("function");
  });

  test("the factory registers the six tools and the zu-todos section", () => {
    const pi = fakePi();
    todosExtensionEntry("sess-1", sandboxAgentDir()).factory({ disabled: new Set(), raw: {} })(pi);
    for (const name of ["todo_list", "todo_add", "todo_update", "todo_promote", "todo_remove", "todo_write"]) {
      expect(pi.tools.has(name)).toBe(true);
    }
    const sections = pi.events.get("before_agent_start") ?? [];
    expect(sections).toHaveLength(1);
    const event = { systemPromptOptions: { sections: {} } };
    (sections[0] as (event: unknown) => void)(event);
    expect((event as { systemPromptOptions: { sections: Record<string, string> } }).systemPromptOptions.sections).toEqual({
      "zu-todos": TODOS_RULE,
    });
  });

  test("the section is byte-stable: two calls write identical text", () => {
    const pi = fakePi();
    todosExtensionEntry("sess-1", sandboxAgentDir()).factory({ disabled: new Set(), raw: {} })(pi);
    const handler = (pi.events.get("before_agent_start") ?? [])[0] as (event: unknown) => void;
    const first: Record<string, string> = {};
    const second: Record<string, string> = {};
    handler({ systemPromptOptions: { sections: first } });
    handler({ systemPromptOptions: { sections: second } });
    expect(first["zu-todos"]).toBe(second["zu-todos"]);
    expect(TODOS_RULE).not.toMatch(/\$\{/); // nothing interpolated, ever
  });

  test("the tools write the store the host channel reads", async () => {
    const agentDir = sandboxAgentDir();
    const sessionId = "sess-todos";
    const pi = fakePi();
    todosExtensionEntry(sessionId, agentDir).factory({ disabled: new Set(), raw: {} })(pi);

    const add = pi.tools.get("todo_add")!;
    const added = (await add.execute("tc-1", { title: "  run   the tests " })) as {
      details: { plan: { todos: Array<{ id: string; title: string; status: string }> } };
    };
    expect(added.details.plan.todos[0]).toMatchObject({ title: "run the tests", status: "pending" });
    const id = added.details.plan.todos[0]!.id;

    // The write landed on disk under the derived root, before any tool ran again.
    const onDisk = readPlan(sessionId, join(agentDir, "..", "zu", "todos"));
    expect(onDisk.todos).toHaveLength(1);

    const update = pi.tools.get("todo_update")!;
    const updated = (await update.execute("tc-2", {
      id,
      status: "done",
      summary: "ran",
      verification: "bun test",
      commitSubject: "test: todos",
    })) as { details: { plan: { todos: Array<{ status: string; summary?: string }> } } };
    expect(updated.details.plan.todos[0]).toMatchObject({ status: "done", summary: "ran" });

    // Promote then writeGroups both reach the same file.
    const promote = pi.tools.get("todo_promote")!;
    const promoted = (await promote.execute("tc-3", { id })) as {
      details: { plan: { groups: Array<{ title: string; todos: Array<{ id: string }> }> } };
    };
    expect(promoted.details.plan.groups).toHaveLength(1);
    expect(promoted.details.plan.groups[0]!.todos[0]!.id).toBe(id);

    const remove = pi.tools.get("todo_remove")!;
    const removed = (await remove.execute("tc-4", { id })) as {
      details: { plan: { groups: unknown[]; todos: unknown[] } };
    };
    expect(removed.details.plan.groups).toHaveLength(0); // the group emptied with the item
    expect(removed.details.plan.todos).toHaveLength(0);

    const write = pi.tools.get("todo_write")!;
    const structured = (await write.execute("tc-5", {
      groups: [{ title: "Ship it", todos: [{ title: "Step one" }] }],
    })) as { details: { plan: { groups: Array<{ title: string }> } } };
    expect(structured.details.plan.groups[0]!.title).toBe("Ship it");

    const list = pi.tools.get("todo_list")!;
    const listed = (await list.execute("tc-6", {})) as { details: { plan: { groups: Array<{ title: string }> } } };
    expect(listed.details.plan.groups[0]!.title).toBe("Ship it");
  });

  test("a refusal answers with the reason's copy and leaves the file alone", async () => {
    const agentDir = sandboxAgentDir();
    const pi = fakePi();
    todosExtensionEntry("sess-bad", agentDir).factory({ disabled: new Set(), raw: {} })(pi);
    const add = pi.tools.get("todo_add")!;
    const result = (await add.execute("tc-1", { title: "Step", group: "g-404" })) as {
      isError?: boolean;
      details: { reason: string };
    };
    expect(result.isError).toBe(true);
    expect(result.details.reason).toBe("unknown-group");
    expect(readPlan("sess-bad", join(agentDir, "..", "zu", "todos")).todos).toHaveLength(0);
  });

  test("the real loader picks the bundled skill up from the entry's skillDirs", async () => {
    const root = mkdtempSync(join(tmpdir(), "zu-todos-load-"));
    roots.push(root);
    const agentDir = join(root, "agent");
    const cwd = join(root, "project");
    mkdirSync(agentDir, { recursive: true });
    mkdirSync(cwd, { recursive: true });
    const entry = todosExtensionEntry("any-session", agentDir);
    const built = await createPiServices({ cwd, agentDir, trusted: true, additionalSkillPaths: entry.skillDirs ?? [] });
    const names = built.services.resourceLoader.getSkills().skills.map((skill) => skill.name);
    expect(names).toContain("todos");
    // The skill's directory is the one this checkout ships, not a stray.
    const skill = built.services.resourceLoader.getSkills().skills.find((candidate) => candidate.name === "todos");
    expect((skill as unknown as { baseDir?: string }).baseDir).toBe(bundledSkillDir("todos"));
  });
});
