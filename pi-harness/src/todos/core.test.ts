/**
 * The plan model's invariants, per issue #13 §6: the single `in_progress`
 * rule, done items staying listed, the writeGroups reconcile, promote's
 * discriminated failures, whitespace compression, the forgiving reader, the
 * atomic writer, and the session-id path guard. Pure functions over temp
 * roots — no pi, no network.
 */

import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  EMPTY_PLAN,
  addTodo,
  applyTodoOp,
  formatPlan,
  groupStatus,
  planPath,
  promoteTodo,
  readPlan,
  removeTodo,
  todosRoot,
  updateTodo,
  writeGroups,
  writePlan,
  type TodoFile,
} from "./core";

const roots: string[] = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function tempRoot(): string {
  const root = mkdtempSync(join(tmpdir(), "zu-todos-"));
  roots.push(root);
  return root;
}

const SESSION = "sess-1";

function planWith(overrides: Partial<TodoFile> = {}): TodoFile {
  return { ...EMPTY_PLAN, ...overrides };
}

/** A plan with two loose drafts and one group of two steps. */
function samplePlan(): TodoFile {
  return planWith({
    todos: [
      { id: "t-1", title: "Draft the API sketch", status: "pending", origin: "agent" },
      { id: "t-2", title: "Ask about the format", status: "in_progress", origin: "user" },
    ],
    groups: [
      {
        id: "g-1",
        title: "Ship the exporter",
        todos: [
          { id: "t-3", title: "Write the module", status: "done", origin: "agent", summary: "exporter.ts lands", verification: "bun test", commitSubject: "feat: exporter" },
          { id: "t-4", title: "Review with the user", status: "pending", origin: "agent" },
        ],
      },
    ],
  });
}

describe("paths", () => {
  test("todosRoot derives ~/.pi/zu/todos from the agent dir", () => {
    expect(todosRoot("/home/u/.pi/agent")).toBe(join("/home/u/.pi", "zu", "todos"));
  });

  test("a session id that is not a path segment is rejected", () => {
    expect(() => planPath("../escape", "/tmp")).toThrow();
    expect(() => planPath("a/b", "/tmp")).toThrow();
    expect(() => planPath("", "/tmp")).toThrow();
    expect(planPath("sess-1", "/tmp")).toBe(join("/tmp", "sess-1.json"));
  });
});

describe("the store", () => {
  test("a missing file reads as an empty plan", () => {
    expect(readPlan(SESSION, tempRoot())).toEqual(EMPTY_PLAN);
  });

  test("a corrupt or wrong-shaped file reads as an empty plan, never throws", () => {
    const root = tempRoot();
    for (const body of ["{ not json", "null", "[]", '{"v":2}', '{"v":1,"todos":{}}', '{"todos":[]}']) {
      writeFileSync(planPath(SESSION, root), body, "utf8");
      expect(readPlan(SESSION, root)).toEqual(EMPTY_PLAN);
    }
  });

  test("a half-shaped entry is dropped, the rest survives", () => {
    const root = tempRoot();
    writeFileSync(
      planPath(SESSION, root),
      JSON.stringify({
        v: 1,
        todos: [{ id: "t-1", title: "Keep me", status: "pending", origin: "user" }, { id: "t-2", title: "  " }, 42],
        groups: [{ id: "g-1", title: "Group", todos: [null, { id: "t-3", title: "Step", status: "weird", origin: "nobody" }] }],
      }),
      "utf8",
    );
    const plan = readPlan(SESSION, root);
    expect(plan.todos.map((todo) => todo.id)).toEqual(["t-1"]);
    expect(plan.groups).toHaveLength(1);
    expect(plan.groups[0]!.todos).toEqual([{ id: "t-3", title: "Step", status: "pending", origin: "agent" }]);
  });

  test("write then read is the same plan, through a temp file and a rename", () => {
    const root = tempRoot();
    const plan = samplePlan();
    writePlan(SESSION, plan, root);
    expect(readPlan(SESSION, root)).toEqual(plan);
    // No temp leftovers beside the plan file.
    expect(existsSync(planPath(SESSION, root) + ".tmp-")).toBe(false);
    expect(readFileSync(planPath(SESSION, root), "utf8").endsWith("\n")).toBe(true);
  });

  test("the plan file lives under <root>/<sessionId>.json", () => {
    const root = tempRoot();
    writePlan(SESSION, samplePlan(), root);
    expect(existsSync(join(root, "sess-1.json"))).toBe(true);
  });
});

describe("addTodo", () => {
  test("a bare title lands in the loose lane with a store-made id", () => {
    const result = addTodo(EMPTY_PLAN, { title: "First draft" });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.file.todos).toHaveLength(1);
    expect(result.file.todos[0]).toMatchObject({ title: "First draft", status: "pending", origin: "agent" });
    expect(result.file.todos[0]!.id).toMatch(/^t-[0-9a-z]+$/);
  });

  test("ids continue from the file's maximum, in base36", () => {
    const file = planWith({
      todos: [{ id: "t-9", title: "Nine", status: "pending", origin: "agent" }],
      groups: [{ id: "g-3", title: "Ship", todos: [{ id: "t-a", title: "Ten", status: "pending", origin: "agent" }] }],
    });
    const result = addTodo(file, { title: "Eleven" });
    if (!result.ok) return;
    expect(result.file.todos.at(-1)!.id).toBe("t-b");
  });

  test("whitespace collapses to single spaces and trims", () => {
    const result = addTodo(EMPTY_PLAN, { title: "  run   the\ttests\nnow  " });
    if (!result.ok) return;
    expect(result.file.todos[0]!.title).toBe("run the tests now");
  });

  test("an empty title is refused; a group id must exist", () => {
    expect(addTodo(EMPTY_PLAN, { title: "   " })).toMatchObject({ ok: false, reason: "empty-title" });
    expect(addTodo(EMPTY_PLAN, { title: "Step", group: "g-404" })).toMatchObject({ ok: false, reason: "unknown-group" });
  });

  test("adding into a group appends to that group only", () => {
    const file = samplePlan();
    const result = addTodo(file, { title: "Polish the README", group: "g-1", origin: "user" });
    if (!result.ok) return;
    expect(result.file.groups[0]!.todos.map((todo) => todo.id)).toEqual(["t-3", "t-4", "t-5"]);
    expect(result.file.todos).toHaveLength(2); // loose lane untouched
    expect(result.file.groups[0]!.todos.at(-1)!.origin).toBe("user");
  });
});

describe("updateTodo", () => {
  test("setting a second in_progress demotes the first, across lane and groups", () => {
    const file = samplePlan(); // t-2 loose is in_progress
    const result = updateTodo(file, { id: "t-4", status: "in_progress" });
    if (!result.ok) return;
    expect(result.file.todos.find((todo) => todo.id === "t-2")!.status).toBe("pending");
    expect(result.file.groups[0]!.todos.find((todo) => todo.id === "t-4")!.status).toBe("in_progress");
  });

  test("done items stay listed; completion fields ride the patch", () => {
    const file = samplePlan();
    const result = updateTodo(file, { id: "t-4", status: "done", summary: "reviewed", verification: "nsh check app" });
    if (!result.ok) return;
    const done = result.file.groups[0]!.todos.find((todo) => todo.id === "t-4")!;
    expect(done.status).toBe("done");
    expect(done.summary).toBe("reviewed");
    expect(done.verification).toBe("nsh check app");
    expect(result.file.groups[0]!.todos).toHaveLength(2);
  });

  test("leaving done clears the completion fields unless the patch restates them", () => {
    const file = samplePlan();
    const reopened = updateTodo(file, { id: "t-3", status: "pending" });
    if (!reopened.ok) return;
    const reopenedTodo = reopened.file.groups[0]!.todos.find((todo) => todo.id === "t-3")!;
    expect(reopenedTodo.summary).toBeUndefined();
    expect(reopenedTodo.verification).toBeUndefined();
    expect(reopenedTodo.commitSubject).toBeUndefined();

    const restated = updateTodo(file, { id: "t-3", status: "pending", summary: "still ours" });
    if (!restated.ok) return;
    const restatedTodo = restated.file.groups[0]!.todos.find((todo) => todo.id === "t-3")!;
    expect(restatedTodo.summary).toBe("still ours");
    expect(restatedTodo.verification).toBeUndefined();
  });

  test("an empty note clears it; unknown ids and bad statuses are refused", () => {
    const file = planWith({ todos: [{ id: "t-1", title: "A", status: "pending", origin: "agent", note: "old" }] });
    const cleared = updateTodo(file, { id: "t-1", note: "" });
    if (!cleared.ok) return;
    expect(cleared.file.todos[0]!.note).toBeUndefined();

    expect(updateTodo(file, { id: "t-404", status: "done" })).toMatchObject({ ok: false, reason: "unknown-id" });
    expect(updateTodo(file, { id: "t-1", status: "later" as never })).toMatchObject({ ok: false, reason: "invalid-status" });
  });
});

describe("promoteTodo", () => {
  test("a loose draft becomes a group titled after it, keeping the id", () => {
    const file = samplePlan();
    const result = promoteTodo(file, "t-1");
    if (!result.ok) return;
    expect(result.file.todos.map((todo) => todo.id)).toEqual(["t-2"]); // only t-1 left the lane
    expect(result.file.groups).toHaveLength(2);
    const group = result.file.groups.at(-1)!;
    expect(group.title).toBe("Draft the API sketch");
    expect(group.todos).toHaveLength(1);
    expect(group.todos[0]!.id).toBe("t-1"); // the same item
    expect(group.todos[0]!.status).toBe("pending");
  });

  test("promoting into a plan whose groups already use that title is refused", () => {
    const file = samplePlan();
    expect(promoteTodo(file, "t-2").ok).toBe(true); // "Ask about the format" is free
    const clash = planWith({
      todos: [{ id: "t-5", title: "Ship the exporter", status: "pending", origin: "agent" }],
      groups: [{ id: "g-1", title: "Ship the exporter", todos: [] }],
    });
    expect(promoteTodo(clash, "t-5")).toMatchObject({ ok: false, reason: "title-collision" });
  });

  test("the four failures are discriminated values", () => {
    const file = samplePlan();
    expect(promoteTodo(file, "t-404")).toMatchObject({ ok: false, reason: "unknown-id" });
    expect(promoteTodo(file, "t-3")).toMatchObject({ ok: false, reason: "grouped-step" });
    const blank = planWith({ todos: [{ id: "t-9", title: "  ", status: "pending", origin: "agent" }] });
    expect(promoteTodo(blank, "t-9")).toMatchObject({ ok: false, reason: "empty-title" });
  });
});

describe("removeTodo", () => {
  test("a loose item is removed; its neighbours stay", () => {
    const file = samplePlan();
    const result = removeTodo(file, "t-1");
    if (!result.ok) return;
    expect(result.file.todos.map((todo) => todo.id)).toEqual(["t-2"]);
    expect(result.file.groups).toHaveLength(1);
  });

  test("removing a group's last step removes the group", () => {
    const file = planWith({
      groups: [
        { id: "g-1", title: "One step only", todos: [{ id: "t-1", title: "The step", status: "pending", origin: "agent" }] },
        { id: "g-2", title: "Stays", todos: [{ id: "t-2", title: "Other", status: "pending", origin: "agent" }] },
      ],
    });
    const result = removeTodo(file, "t-1");
    if (!result.ok) return;
    expect(result.file.groups.map((group) => group.id)).toEqual(["g-2"]);
  });

  test("removing another item leaves an intentionally empty group alone", () => {
    const result = removeTodo(
      planWith({
        todos: [{ id: "t-1", title: "Draft", status: "pending", origin: "agent" }],
        groups: [{ id: "g-1", title: "Planning…", todos: [] }],
      }),
      "t-1",
    );
    if (!result.ok) return;
    expect(result.file.todos).toEqual([]);
    expect(result.file.groups).toEqual([{ id: "g-1", title: "Planning…", todos: [] }]);
  });

  test("an unknown id is refused", () => {
    expect(removeTodo(samplePlan(), "t-404")).toMatchObject({ ok: false, reason: "unknown-id" });
  });
});

describe("writeGroups (reconcile)", () => {
  test("listed ids keep their status; a status in the input is ignored", () => {
    const file = samplePlan();
    const result = writeGroups(file, {
      groups: [
        { id: "g-1", title: "Ship the exporter", todos: [{ id: "t-3", title: "ignored" }, { id: "t-4", title: "ignored" }] },
      ],
    });
    if (!result.ok) return;
    const done = result.file.groups[0]!.todos.find((todo) => todo.id === "t-3")!;
    expect(done.status).toBe("done");
    expect(done.title).toBe("Write the module"); // the stored title wins for known ids
    expect(done.summary).toBe("exporter.ts lands");
    expect(result.file.todos.map((todo) => todo.id)).toEqual(["t-1", "t-2"]); // loose lane untouched
  });

  test("unknown ids become new pending steps and missing grouped ids are removed", () => {
    const file = samplePlan(); // g-1 holds t-3, t-4
    const result = writeGroups(file, {
      groups: [
        { id: "g-1", title: "Ship the exporter", todos: [{ id: "t-fresh-9", title: "New step" }] },
      ],
    });
    if (!result.ok) return;
    expect(result.file.groups[0]!.todos).toHaveLength(1);
    expect(result.file.groups[0]!.todos[0]).toMatchObject({ id: "t-fresh-9", title: "New step", status: "pending" });
    // t-3 and t-4 were not listed: they left the plan. Loose drafts survived.
    expect(result.file.todos).toHaveLength(2);
  });

  test("an id-less item mints a fresh id; an empty group is a legal group", () => {
    const file = samplePlan();
    const result = writeGroups(file, {
      groups: [{ id: "g-1", title: "Ship the exporter" }, { title: "Brand new group", todos: [{ title: "Fresh step" }] }],
    });
    if (!result.ok) return;
    expect(result.file.groups).toHaveLength(2);
    expect(result.file.groups[0]!.todos).toEqual([]); // listed empty, stays empty
    const fresh = result.file.groups[1]!;
    expect(fresh.id).toMatch(/^g-[0-9a-z]+$/);
    expect(fresh.todos[0]!.id).toMatch(/^t-[0-9a-z]+$/);
    expect(fresh.todos[0]!.status).toBe("pending");
  });

  test("an item moves between groups, keeping its state; a loose id is refused", () => {
    const file = samplePlan();
    const moved = writeGroups(file, {
      groups: [
        { id: "g-1", title: "Ship the exporter", todos: [{ title: "Only this" }] },
        { title: "Second pass", todos: [{ id: "t-3", title: "ignored" }] },
      ],
    });
    if (!moved.ok) return;
    expect(moved.file.groups[1]!.todos[0]!.id).toBe("t-3");
    expect(moved.file.groups[1]!.todos[0]!.status).toBe("done"); // state preserved across the move
    expect(moved.file.groups[0]!.todos).toHaveLength(1);

    expect(
      writeGroups(file, { groups: [{ title: "No smuggling", todos: [{ id: "t-1", title: "loose" }] }] }),
    ).toMatchObject({ ok: false, reason: "loose-item" });
  });

  test("renaming an existing group works; duplicate titles and unknown group ids refuse", () => {
    const file = samplePlan();
    const renamed = writeGroups(file, { groups: [{ id: "g-1", title: "Export, shipped", todos: [{ id: "t-3", title: "x" }] }] });
    if (!renamed.ok) return;
    expect(renamed.file.groups[0]!.title).toBe("Export, shipped");

    expect(
      writeGroups(file, { groups: [{ title: "Ship the exporter" }] }),
    ).toMatchObject({ ok: false, reason: "title-collision" });
    expect(
      writeGroups(file, { groups: [{ title: "A" }, { title: "A" }] }),
    ).toMatchObject({ ok: false, reason: "title-collision" });
    expect(
      writeGroups(file, { groups: [{ id: "g-404", title: "Nowhere" }] }),
    ).toMatchObject({ ok: false, reason: "unknown-group" });
    expect(writeGroups(file, { groups: [{ title: "   " }] })).toMatchObject({ ok: false, reason: "empty-title" });
  });

  test("a group id used as a step id is refused without changing the file", () => {
    const file = planWith({
      groups: [{ id: "g-1", title: "Ship", todos: [{ id: "t-1", title: "Draft", status: "pending", origin: "agent" }] }],
    });
    const result = writeGroups(file, {
      groups: [{ id: "g-1", title: "Ship", todos: [{ id: "g-1", title: "Bogus step" }] }],
    });
    expect(result).toMatchObject({ ok: false, reason: "invalid-id" });
    expect(file.groups[0]!.todos.map((todo) => todo.id)).toEqual(["t-1"]);
    expect(formatPlan(file)).toContain("Draft");
  });

  test("the same file item cannot be listed twice; generated ids stay unique", () => {
    const file = samplePlan();
    expect(
      writeGroups(file, {
        groups: [
          { title: "A", todos: [{ id: "t-3", title: "x" }] },
          { title: "B", todos: [{ id: "t-3", title: "x" }] },
        ],
      }),
    ).toMatchObject({ ok: false, reason: "invalid-id" });

    const minted = writeGroups(file, { groups: [{ title: "A", todos: [{ title: "one" }, { title: "two" }] }] });
    if (!minted.ok) return;
    const [first, second] = minted.file.groups[0]!.todos;
    expect(first!.id).not.toBe(second!.id);
    expect(minted.file.groups[0]!.id).not.toBe("g-1"); // g-1 was not mentioned; its id is not reused
  });
});

describe("groupStatus", () => {
  test("derived, never stored: empty pending, all done, any active, else pending", () => {
    expect(groupStatus({ id: "g-1", title: "t", todos: [] })).toBe("pending");
    expect(groupStatus({ id: "g-1", title: "t", todos: [{ id: "t-1", title: "a", status: "done", origin: "agent" }] })).toBe("done");
    expect(
      groupStatus({
        id: "g-1",
        title: "t",
        todos: [
          { id: "t-1", title: "a", status: "done", origin: "agent" },
          { id: "t-2", title: "b", status: "in_progress", origin: "agent" },
        ],
      }),
    ).toBe("active");
    expect(
      groupStatus({
        id: "g-1",
        title: "t",
        todos: [
          { id: "t-1", title: "a", status: "pending", origin: "agent" },
          { id: "t-2", title: "b", status: "done", origin: "agent" },
        ],
      }),
    ).toBe("pending");
  });
});

describe("applyTodoOp (the wire envelope)", () => {
  test("each kind runs against the file on disk and persists its result", () => {
    const root = tempRoot();
    expect(applyTodoOp(SESSION, root, { kind: "add", title: "First" })).toMatchObject({ ok: true });
    expect(readPlan(SESSION, root).todos[0]!.title).toBe("First");

    const id = readPlan(SESSION, root).todos[0]!.id;
    expect(applyTodoOp(SESSION, root, { kind: "update", id, status: "done", summary: "s" })).toMatchObject({ ok: true });
    expect(readPlan(SESSION, root).todos[0]!.status).toBe("done");

    expect(applyTodoOp(SESSION, root, { kind: "promote", id })).toMatchObject({ ok: true });
    expect(readPlan(SESSION, root).groups).toHaveLength(1);
    expect(readPlan(SESSION, root).todos).toHaveLength(0);

    expect(applyTodoOp(SESSION, root, { kind: "add", title: "Second", origin: "user" })).toMatchObject({ ok: true });
    const second = readPlan(SESSION, root).todos[0]!;
    expect(second.origin).toBe("user");
    expect(applyTodoOp(SESSION, root, { kind: "remove", id: second.id })).toMatchObject({ ok: true });
    expect(readPlan(SESSION, root).todos).toHaveLength(0);

    expect(
      applyTodoOp(SESSION, root, { kind: "writeGroups", groups: [{ title: "New layout", todos: [{ title: "Step" }] }] }),
    ).toMatchObject({ ok: true });
    expect(readPlan(SESSION, root).groups[0]!.title).toBe("New layout");
  });

  test("a malformed op is invalid-op and a failing op writes nothing", () => {
    const root = tempRoot();
    writePlan(SESSION, samplePlan(), root);
    for (const op of [null, undefined, "add", {}, { kind: "migrate" }, { kind: "writeGroups", groups: "no" }]) {
      expect(applyTodoOp(SESSION, root, op)).toMatchObject({ ok: false, reason: "invalid-op" });
    }
    expect(applyTodoOp(SESSION, root, { kind: "promote", id: "t-404" })).toMatchObject({ ok: false, reason: "unknown-id" });
    expect(readPlan(SESSION, root)).toEqual(samplePlan()); // untouched by the failed ops
  });
});

describe("formatPlan", () => {
  test("the compact text mirrors the file: loose drafts, then groups with state", () => {
    const text = formatPlan(samplePlan());
    expect(text).toContain("loose drafts:");
    expect(text).toContain("[ ] t-1 Draft the API sketch");
    expect(text).toContain("[~] t-2 Ask about the format");
    expect(text).toContain('group g-1 "Ship the exporter" — pending');
    expect(text).toContain("[x] t-3 Write the module — verified: bun test");
    expect(formatPlan(EMPTY_PLAN)).toBe("the plan is empty");
  });
});
