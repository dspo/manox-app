/**
 * The plan steering (A2): every panel edit reaches the model as a note, a
 * running turn is woken at its settle, a removal interrupts a running turn
 * first, and the user's own stop never gets overruled by a wake.
 */

import { describe, expect, test } from "bun:test";
import { EMPTY_PLAN, type TodoFile } from "./core";
import { PlanSteer, formatPlanNotes, noteFor, planEditFacts, type PlanSteerPorts } from "./steer";

/** A plan with one loose draft and one in_progress step. */
function planWith(overrides: Partial<TodoFile> = {}): TodoFile {
  return { ...EMPTY_PLAN, ...overrides };
}

function fakePorts(initial = { idle: true }): {
  ports: PlanSteerPorts;
  injected: Array<{ text: string; wake: boolean }>;
  aborts(): number;
} {
  const injected: Array<{ text: string; wake: boolean }> = [];
  let aborts = 0;
  const ports: PlanSteerPorts = {
    idle: () => initial.idle,
    abort: async () => {
      aborts += 1;
      initial.idle = true;
    },
    inject: async (text, options) => {
      injected.push({ text, wake: options.wake });
    },
  };
  return { ports, injected, aborts: () => aborts };
}

const SAMPLE = planWith({
  todos: [{ id: "t-1", title: "Draft the sketch", status: "pending", origin: "agent" }],
  groups: [
    { id: "g-1", title: "Ship it", todos: [{ id: "t-2", title: "Write the module", status: "in_progress", origin: "agent" }] },
  ],
});

describe("noteFor", () => {
  test("each kind gets its own line; a removal carries the drop instruction", () => {
    expect(noteFor({ kind: "add", title: "A" })).toBe(`added "A"`);
    expect(noteFor({ kind: "update", title: "A", status: "done" })).toBe(`marked "A" done`);
    expect(noteFor({ kind: "update", title: "A", status: "pending" })).toBe(`reopened "A" — it needs work again`);
    expect(noteFor({ kind: "promote", title: "A" })).toBe(`promoted "A" into a task group`);
    expect(noteFor({ kind: "remove", title: "A" })).toContain(`removed "A"`);
    expect(noteFor({ kind: "remove", title: "A" })).toContain("do not re-add it");
    expect(noteFor({ kind: "writeGroups" })).toBe("restructured the task groups");
    expect(noteFor({ kind: "migrate" })).toBeNull();
  });

  test("the batched note lists the edits and the standing instruction", () => {
    const text = formatPlanNotes([`added "A"`, `removed "B" — it is gone from the plan`]);
    expect(text).toContain("The user just edited the shared TODO plan:");
    expect(text).toContain(`- added "A"`);
    expect(text).toContain("- removed");
    expect(text).toContain("Re-read the plan (todo_list) before you continue.");
  });
});

describe("planEditFacts", () => {
  test("titles resolve against the plan before the op", () => {
    const facts = planEditFacts({ kind: "remove", id: "t-2" }, SAMPLE);
    expect(facts).toEqual({ kind: "remove", title: "Write the module" });
    expect(planEditFacts({ kind: "add", title: "New" }, SAMPLE)).toEqual({ kind: "add", title: "New" });
    expect(planEditFacts({ kind: "update", id: "t-1", status: "done" }, SAMPLE)).toEqual({
      kind: "update",
      title: "Draft the sketch",
      status: "done",
    });
    expect(planEditFacts({ kind: "promote", id: "t-404" }, SAMPLE)).toEqual({ kind: "promote" });
    expect(planEditFacts("garbage", EMPTY_PLAN)).toEqual({ kind: "" });
  });
});

describe("PlanSteer", () => {
  test("an idle session's edit injects now and wakes the model", async () => {
    const { ports, injected, aborts } = fakePorts();
    const steer = new PlanSteer(ports);
    await steer.afterEdit(planEditFacts({ kind: "promote", id: "t-1" }, SAMPLE));
    expect(aborts()).toBe(0);
    expect(injected).toHaveLength(1);
    expect(injected[0]).toMatchObject({ wake: true });
    expect(injected[0]!.text).toContain(`promoted "Draft the sketch" into a task group`);
  });

  test("an edit during a running turn queues; the settle flushes it", async () => {
    const state = { idle: false };
    const { ports, injected, aborts } = fakePorts(state);
    const steer = new PlanSteer(ports);
    await steer.afterEdit(planEditFacts({ kind: "add", title: "New" }, SAMPLE));
    expect(injected).toHaveLength(0); // nothing mid-turn
    expect(aborts()).toBe(0); // a plain edit does not interrupt
    // The turn settles on its own: the note wakes the model to adapt.
    state.idle = true;
    await steer.flush(true);
    expect(injected).toHaveLength(1);
    expect(injected[0]).toMatchObject({ wake: true, text: expect.stringContaining(`added "New"`) });
  });

  test("queued notes batch into one message, not one turn per click", async () => {
    const state = { idle: false };
    const { ports, injected } = fakePorts(state);
    const steer = new PlanSteer(ports);
    await steer.afterEdit(planEditFacts({ kind: "add", title: "A" }, SAMPLE));
    await steer.afterEdit(planEditFacts({ kind: "update", id: "t-2", status: "done" }, SAMPLE));
    state.idle = true;
    await steer.flush(true);
    expect(injected).toHaveLength(1);
    expect(injected[0]!.text).toContain(`- added "A"`);
    expect(injected[0]!.text).toContain(`- marked "Write the module" done`);
  });

  test("a removal while the model runs interrupts the turn first", async () => {
    const state = { idle: false };
    const { ports, injected, aborts } = fakePorts(state);
    const steer = new PlanSteer(ports);
    await steer.afterEdit(planEditFacts({ kind: "remove", id: "t-2" }, SAMPLE));
    expect(aborts()).toBe(1); // the deleted item must not be finished
    expect(injected).toHaveLength(1); // idle again after the abort: the note goes out now
    expect(injected[0]).toMatchObject({ wake: true });
    expect(injected[0]!.text).toContain(`removed "Write the module"`);
    expect(injected[0]!.text).toContain("do not keep working on it");
  });

  test("a removal while idle does not abort", async () => {
    const { ports, injected, aborts } = fakePorts();
    const steer = new PlanSteer(ports);
    await steer.afterEdit(planEditFacts({ kind: "remove", id: "t-1" }, SAMPLE));
    expect(aborts()).toBe(0);
    expect(injected).toHaveLength(1);
  });

  test("the user's stop flushes queued notes without waking", async () => {
    const state = { idle: false };
    const { ports, injected } = fakePorts(state);
    const steer = new PlanSteer(ports);
    await steer.afterEdit(planEditFacts({ kind: "add", title: "A" }, SAMPLE));
    state.idle = true;
    await steer.flush(false); // agent_settled { aborted: true }
    expect(injected).toHaveLength(1);
    expect(injected[0]).toMatchObject({ wake: false });
  });

  test("a failed inject logs and does not throw", async () => {
    const logged: string[] = [];
    const steer = new PlanSteer({
      idle: () => true,
      abort: async () => {},
      inject: async () => {
        throw new Error("wire down");
      },
      log: (line) => logged.push(line),
    });
    await steer.afterEdit({ kind: "add", title: "A" });
    expect(logged).toHaveLength(1);
    expect(logged[0]).toContain("inject failed");
  });
});
