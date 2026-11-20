/**
 * The plan's six tools (`todo_*`), the byte-stable `zu-todos` prompt
 * section, and the tool error copy (A2, issue #13).
 *
 * Clean room: written against the issue SPEC only. The tool names are the
 * model interface and stay `todo_*` regardless of the implementation.
 *
 * Every tool runs the same read-op-write stretch against
 * `~/.pi/zu/todos/<sessionId>.json` — the same store the host channel
 * (`_zu/todos/*`) serves — so an edit the user just made in the panel is
 * what the next `todo_list` reads, and a tool's write is what the panel's
 * next poll shows. No cache anywhere.
 *
 * The section is bound to this extension on purpose: turning `todos` off in
 * `disabledExtensions` removes the tools, the skill, and the rule together,
 * so the prompt never advertises a mechanism the session does not have.
 * Like every section its text is a module-level const — it rides each
 * request, and anything interpolated would invalidate the provider's
 * prefix cache turn after turn.
 */

import { Type } from "typebox";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { promptSectionExtension } from "../extensions/prompt-section";
import {
  TODO_FAILURE_TEXT,
  addTodo,
  formatPlan,
  promoteTodo,
  readPlan,
  removeTodo,
  todosRoot,
  updateTodo,
  writeGroups,
  writePlan,
  type TodoFile,
  type TodoOpResult,
} from "./core";

/**
 * The captain's plan rule. Rewritten for Zu: the section only has to make
 * the model aware that a shared plan may exist and where its discipline
 * lives — the skill carries the rest (when to create one, granularity,
 * completion records).
 */
export const TODOS_RULE =
  "This conversation may have a shared TODO plan the user can see and edit at any time. Read the todos skill before you create or restructure one.";

/** Where the tools find the session and its state root. */
export interface TodosContext {
  sessionId: string;
  agentDir: string;
}

/** One tool's result: compact text up front, the full plan in `details`. */
function planResult(file: TodoFile, note?: string) {
  return {
    content: [{ type: "text" as const, text: note ? `${note}\n${formatPlan(file)}` : formatPlan(file) }],
    details: { plan: file },
  };
}

/** A refusal: the reason's shared copy, plus the plan as it stands. */
function planError(result: Extract<TodoOpResult, { ok: false }>, file: TodoFile) {
  return {
    content: [{ type: "text" as const, text: `${TODO_FAILURE_TEXT[result.reason]} (${result.reason})` }],
    details: { plan: file, reason: result.reason },
    isError: true as const,
  };
}

/** Read → op → write, one synchronous stretch; a failed op writes nothing. */
function runToolOp(context: TodosContext, op: (file: TodoFile) => TodoOpResult) {
  const root = todosRoot(context.agentDir);
  const file = readPlan(context.sessionId, root);
  const result = op(file);
  if (result.ok) writePlan(context.sessionId, result.file, root);
  return result.ok ? planResult(result.file) : planError(result, file);
}

/**
 * The six tools and the section, registered on the captain's session only
 * (subagents never load registry extensions).
 */
export function todosTools(context: TodosContext) {
  return (pi: ExtensionAPI): void => {
    promptSectionExtension("zu-todos", TODOS_RULE)(pi);

    pi.registerTool({
      name: "todo_list",
      label: "Read the plan",
      description:
        "Read the conversation's TODO plan: loose drafts, then task groups with their steps. The user can edit the plan (or erase items) at any time, so re-read it before you pick the next step or act on what you remember from an earlier read.",
      promptSnippet: "read the shared TODO plan",
      parameters: Type.Object({}),
      async execute() {
        const root = todosRoot(context.agentDir);
        return planResult(readPlan(context.sessionId, root));
      },
    });

    pi.registerTool({
      name: "todo_add",
      label: "Add a plan item",
      description:
        "Add one item to the plan. Without a group it becomes a loose draft (a shared sketch either side can promote later); with an existing group id it becomes that group's next step. Titles are single lines. The user can remove anything you add — do not re-add items the user removed.",
      promptSnippet: "add an item to the plan",
      parameters: Type.Object({
        title: Type.String(),
        group: Type.Optional(Type.String()),
      }),
      async execute(_toolCallId: string, params: any) {
        return runToolOp(context, (file) =>
          addTodo(file, {
            title: String(params?.title ?? ""),
            ...(typeof params?.group === "string" ? { group: params.group } : {}),
          }),
        );
      },
    });

    pi.registerTool({
      name: "todo_update",
      label: "Update a plan item",
      description:
        "Update one item by id. Only one item may be in_progress at a time — setting one demotes the other, so take one step at a time. A done item stays in the plan; reopening one drops its completion record unless your patch restates it. When you complete a step, write summary, verification (a check you actually ran — never claim one you did not run) and, for code changes, a commit subject.",
      promptSnippet: "update a plan item's status or completion",
      parameters: Type.Object({
        id: Type.String(),
        status: Type.Optional(Type.Union([Type.Literal("pending"), Type.Literal("in_progress"), Type.Literal("done")])),
        note: Type.Optional(Type.String()),
        summary: Type.Optional(Type.String()),
        verification: Type.Optional(Type.String()),
        commitSubject: Type.Optional(Type.String()),
      }),
      async execute(_toolCallId: string, params: any) {
        return runToolOp(context, (file) =>
          updateTodo(file, {
            id: String(params?.id ?? ""),
            ...(params?.status === "pending" || params?.status === "in_progress" || params?.status === "done"
              ? { status: params.status }
              : {}),
            ...(typeof params?.note === "string" ? { note: params.note } : {}),
            ...(typeof params?.summary === "string" ? { summary: params.summary } : {}),
            ...(typeof params?.verification === "string" ? { verification: params.verification } : {}),
            ...(typeof params?.commitSubject === "string" ? { commitSubject: params.commitSubject } : {}),
          }),
        );
      },
    });

    pi.registerTool({
      name: "todo_promote",
      label: "Promote a draft",
      description:
        "The only path from the loose lane into a task group: the draft becomes the first step of a new group titled after it, keeping its id. A draft that already belongs to a group is refused — group steps move with todo_write, drafts move with this tool.",
      promptSnippet: "promote a loose draft into a task group",
      parameters: Type.Object({ id: Type.String() }),
      async execute(_toolCallId: string, params: any) {
        return runToolOp(context, (file) => promoteTodo(file, String(params?.id ?? "")));
      },
    });

    pi.registerTool({
      name: "todo_remove",
      label: "Remove a plan item",
      description:
        "Remove exactly one item by id; a group it empties is removed with it. Use it for your own obsolete steps — if the user removed something, leave it removed.",
      promptSnippet: "remove one plan item",
      parameters: Type.Object({ id: Type.String() }),
      async execute(_toolCallId: string, params: any) {
        return runToolOp(context, (file) => removeTodo(file, String(params?.id ?? "")));
      },
    });

    pi.registerTool({
      name: "todo_write",
      label: "Restructure the plan",
      description:
        "Reconcile the task groups with the structure you send. Known group ids may be renamed; item ids keep their status (a status in your input is ignored — this is not how you complete steps) and item titles are preserved; unknown item ids become new pending steps; grouped ids you omit are removed; a group you list with no steps stays as an empty group. The loose lane is never touched here — drafts move with todo_promote. Send every group you want to keep; this is a rebuild by id, not a patch on top.",
      promptSnippet: "restructure the plan's task groups",
      parameters: Type.Object({
        groups: Type.Array(
          Type.Object({
            id: Type.Optional(Type.String()),
            title: Type.String(),
            todos: Type.Optional(Type.Array(Type.Object({ id: Type.Optional(Type.String()), title: Type.String() }))),
          }),
        ),
      }),
      async execute(_toolCallId: string, params: any) {
        return runToolOp(context, (file) => writeGroups(file, { groups: (params?.groups ?? []) as any[] }));
      },
    });
  };
}

/** The `todo_*` tool names, for the approval gate's read-only list and tests. */
export const TODO_TOOL_NAMES = [
  "todo_list",
  "todo_add",
  "todo_update",
  "todo_promote",
  "todo_remove",
  "todo_write",
] as const;
