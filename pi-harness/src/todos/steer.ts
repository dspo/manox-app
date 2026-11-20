/**
 * What a user's plan edit tells the model (A2).
 *
 * The panel is the only caller of `_zu/todos/write`, so every successful
 * write there is a user edit the model does not know about — it last saw the
 * plan through its own `todo_*` calls. Two steering rules:
 *
 * - every edit becomes a note in the conversation naming what changed, and
 *   when the session is between turns the note also wakes one, so the model
 *   re-reads the plan now instead of acting on what it remembers;
 * - a removal goes further: a turn that is running may be executing exactly
 *   the item the user just deleted, so the turn is interrupted first. The
 *   note then tells the model the item is gone and must not come back.
 *
 * Edits made while a turn runs queue their notes; they go out when the turn
 * settles (`agent_settled`). A settle the user caused themselves (abort)
 * flushes without waking — the user said stop, and a plan note must not
 * overrule that by starting a new turn.
 */

import type { TodoFile } from "./core";

/** The custom entry type a plan note rides (`display: true`, in context). */
export const PLAN_NOTE_TYPE = "zu.plan.note";

/** What the harness needs to steer: the session's idle state, its abort,
 * and one inject that can wake the model. */
export interface PlanSteerPorts {
  /** Whether the session's model is between turns. */
  idle(): boolean;
  /** Interrupts the running turn. */
  abort(): Promise<void>;
  /** Puts one note into the conversation; `wake` also starts a turn. */
  inject(text: string, options: { wake: boolean }): Promise<void>;
  /** One line for the wire log. */
  log?(line: string): void;
}

/** The facts one panel edit contributes to a note. */
export interface PlanEditFacts {
  kind: string;
  /** The affected item's title, resolved from the plan before the op. */
  title?: string;
  /** For an `update`: the status the user set (done / pending). */
  status?: string;
}

/** One bullet, per edit kind. A removal's bullet carries the instruction —
 * the model must drop the item, not mourn it. */
export function noteFor(edit: PlanEditFacts): string | null {
  const title = edit.title ? ` "${edit.title}"` : "";
  switch (edit.kind) {
    case "add":
      return `added${title}`;
    case "update":
      if (edit.status === "done") return `marked${title} done`;
      if (edit.status === "pending") return `reopened${title} — it needs work again`;
      return `updated${title}`;
    case "promote":
      return `promoted${title} into a task group`;
    case "remove":
      return `removed${title} — it is gone from the plan; do not re-add it and do not keep working on it`;
    case "writeGroups":
      return "restructured the task groups";
    default:
      return null;
  }
}

/** The whole note: the edits as bullets, then the standing instruction. */
export function formatPlanNotes(notes: string[]): string {
  return [
    "The user just edited the shared TODO plan:",
    ...notes.map((note) => `- ${note}`),
    "Re-read the plan (todo_list) before you continue.",
  ].join("\n");
}

/** Extracts the note facts from a wire op, resolving titles against the plan
 * as it was before the op ran (a removal's title only exists there). */
export function planEditFacts(op: unknown, before: TodoFile): PlanEditFacts {
  if (typeof op !== "object" || op === null) return { kind: "" };
  const record = op as Record<string, unknown>;
  const kind = typeof record.kind === "string" ? record.kind : "";
  const titleById = (id: string): string | undefined => {
    for (const todo of before.todos) if (todo.id === id) return todo.title;
    for (const group of before.groups) {
      for (const todo of group.todos) if (todo.id === id) return todo.title;
    }
    return undefined;
  };
  switch (kind) {
    case "add":
      return { kind, ...(typeof record.title === "string" ? { title: record.title } : {}) };
    case "update": {
      const id = typeof record.id === "string" ? record.id : "";
      return {
        kind,
        ...(titleById(id) ? { title: titleById(id) } : {}),
        ...(typeof record.status === "string" ? { status: record.status } : {}),
      };
    }
    case "promote":
    case "remove": {
      const id = typeof record.id === "string" ? record.id : "";
      return { kind, ...(titleById(id) ? { title: titleById(id) } : {}) };
    }
    default:
      return { kind };
  }
}

/** The steering state of one session. */
export class PlanSteer {
  private pending: string[] = [];

  constructor(private readonly ports: PlanSteerPorts) {}

  /**
   * Records one successful panel edit and steers. Idle → the note goes out
   * now and wakes the model. Running → the note queues (the settle flushes
   * it), except a removal, which interrupts the turn first so the model
   * cannot finish work on the deleted item.
   */
  async afterEdit(edit: PlanEditFacts): Promise<void> {
    const note = noteFor(edit);
    if (!note) return;
    if (edit.kind === "remove" && !this.ports.idle()) {
      try {
        await this.ports.abort();
      } catch (error) {
        this.ports.log?.(`plan steer: abort failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    this.pending.push(note);
    if (this.ports.idle()) await this.flush(true);
  }

  /**
   * Sends the queued notes as one conversation message. `wake` decides
   * whether the model is woken to react now (a turn that settled on its own)
   * or the note only joins the context (the user stopped the turn).
   */
  async flush(wake: boolean): Promise<void> {
    if (this.pending.length === 0) return;
    const notes = this.pending.splice(0);
    try {
      await this.ports.inject(formatPlanNotes(notes), { wake });
    } catch (error) {
      this.ports.log?.(`plan steer: inject failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}
