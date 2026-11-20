/**
 * The session TODO plan: data model, store, and the pure operations the
 * tools and the host channel both run (A2, issue #13).
 *
 * One plan per pi session at `~/.pi/zu/todos/<sessionId>.json`. Two levels:
 * a shared loose lane (drafts that exist before a task takes shape) and
 * groups (one user request's steps). There is no priority and no persisted
 * group status — a group's state is derived from its items.
 *
 * The store has no cache: every operation re-reads the file, because the
 * user edits the plan from the panel while a turn runs, and the model must
 * see the user's edit on its next `todo_list`. Writes go through a temp file
 * and a rename, so a reader never sees a half-written plan.
 *
 * This file is pi-free on purpose: `bun test` exercises it without the
 * harness, and `src/acp/app.ts` may import from it without pulling pi in.
 * All mutating operations are total: they take a `TodoFile` in and return a
 * discriminated result, so a bad id or title is a value the caller formats,
 * not an exception to catch at every call site.
 */

import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// --- model ------------------------------------------------------------------

export type TodoStatus = "pending" | "in_progress" | "done";
export type TodoOrigin = "agent" | "user";

export interface Todo {
  /** `t-<base36>`; promote keeps the id, so a step stays addressable. */
  id: string;
  /** One line: whitespace is compressed to single spaces before storing. */
  title: string;
  status: TodoStatus;
  /** Who wrote the item; the panel defaults its own adds to `user`. */
  origin: TodoOrigin;
  /** The agent's working details; the panel does not edit it (v1). */
  note?: string;
  /** Set on completion: what happened. */
  summary?: string;
  /** Set on completion: the check that actually ran. */
  verification?: string;
  /** Set on completion: a usable commit subject, when the step made one. */
  commitSubject?: string;
}

export interface TodoGroup {
  /** `g-<base36>`. */
  id: string;
  /** One user request's outcome, never "step 1". */
  title: string;
  todos: Todo[];
}

export interface TodoFile {
  v: 1;
  /** The loose lane: drafts shared by user and agent before grouping. */
  todos: Todo[];
  groups: TodoGroup[];
}

/** The plan a session starts with (also what a missing or corrupt file reads as). */
export const EMPTY_PLAN: TodoFile = { v: 1, todos: [], groups: [] };

const STATUSES: readonly TodoStatus[] = ["pending", "in_progress", "done"];

/** Session ids are path segments here; anything else is rejected outright. */
const SESSION_ID_PATTERN = /^[A-Za-z0-9._-]+$/;

/** The id scheme the store generates and honors from `todo_write`. */
const ID_PATTERN = /^t-[A-Za-z0-9]+$/;
const GROUP_ID_PATTERN = /^g-[A-Za-z0-9]+$/;

// --- failures ---------------------------------------------------------------

/**
 * Why an operation refused. Each value has tool error copy and a wire error
 * code (`_zu/todos/write` answers `invalid_params` with this as `data.code`),
 * so a client or a model sees the same vocabulary everywhere.
 */
export type TodoOpReason =
  /** The op envelope itself is unreadable (unknown kind, missing fields). */
  | "invalid-op"
  /** A client-supplied id breaks the scheme or duplicates an existing one. */
  | "invalid-id"
  | "invalid-status"
  | "unknown-id"
  | "unknown-group"
  /** `todo_promote` on an item that already belongs to a group. */
  | "grouped-step"
  | "empty-title"
  | "title-collision"
  /** `todo_write` listed a loose draft inside a group; promote is the only
   * loose→group path, so a restructure cannot smuggle one in. */
  | "loose-item";

export type TodoOpResult =
  | { ok: true; file: TodoFile }
  | { ok: false; reason: TodoOpReason };

/** One line of English per failure; shared by the tools and the wire. */
export const TODO_FAILURE_TEXT: Record<TodoOpReason, string> = {
  "invalid-op": "the operation is malformed (unknown kind or missing fields)",
  "invalid-id": "the id does not fit the plan's id scheme or duplicates an existing one",
  "invalid-status": "the status is not one of pending, in_progress, done",
  "unknown-id": "no item carries that id",
  "unknown-group": "no group carries that id",
  "grouped-step": "that item is already a step of a group; only loose drafts can be promoted",
  "empty-title": "the title is empty after collapsing whitespace",
  "title-collision": "a group with that title already exists",
  "loose-item": "that id belongs to the loose lane; use todo_promote to move it into a group",
};

// --- ids --------------------------------------------------------------------

/** The largest numeric suffix among `prefix-<n>` ids, or 0. */
function maxIdSuffix(ids: Iterable<string>, prefix: string): number {
  let max = 0;
  for (const id of ids) {
    if (!id.startsWith(`${prefix}-`)) continue;
    const suffix = id.slice(prefix.length + 1);
    if (!/^[0-9]+$/.test(suffix)) continue;
    max = Math.max(max, Number.parseInt(suffix, 10));
  }
  return max;
}

/** Every item and group id in the file. */
function allIds(file: TodoFile): Set<string> {
  const ids = new Set<string>();
  for (const todo of file.todos) ids.add(todo.id);
  for (const group of file.groups) {
    ids.add(group.id);
    for (const todo of group.todos) ids.add(todo.id);
  }
  return ids;
}

/**
 * The next `prefix-<base36>` id. Sequential (t-1, t-2, …, t-a) so plan files
 * stay readable; the scan restarts at the current maximum, and a client
 * id that already claimed the slot just moves to the next one.
 */
function nextId(file: TodoFile, prefix: string): string {
  const taken = allIds(file);
  let n = maxIdSuffix(taken, prefix) + 1;
  let id = `${prefix}-${n.toString(36)}`;
  while (taken.has(id)) {
    n += 1;
    id = `${prefix}-${n.toString(36)}`;
  }
  return id;
}

// --- paths ------------------------------------------------------------------

/**
 * `~/.pi/zu/todos`, derived from the agent dir exactly like the settings and
 * subagent stores do (`join(agentDir, "..", "zu")`). Tests pass their own
 * root; production always uses this derivation, so the tools and the host
 * channel cannot drift apart.
 */
export function todosRoot(agentDir: string): string {
  return join(agentDir, "..", "zu", "todos");
}

/** `<root>/<sessionId>.json`; throws on a session id that is not a path segment. */
export function planPath(sessionId: string, root: string): string {
  if (!SESSION_ID_PATTERN.test(sessionId)) {
    throw new Error(`invalid session id for a plan path: ${JSON.stringify(sessionId)}`);
  }
  return join(root, `${sessionId}.json`);
}

// --- store ------------------------------------------------------------------

/** One item as the file may hold it; fields that are not strings drop. */
function coerceTodo(raw: unknown): Todo | null {
  if (typeof raw !== "object" || raw === null) return null;
  const record = raw as Record<string, unknown>;
  const id = typeof record.id === "string" ? record.id : "";
  const title = typeof record.title === "string" ? compress(record.title) : "";
  if (!id || !title) return null;
  const status = STATUSES.includes(record.status as TodoStatus) ? (record.status as TodoStatus) : "pending";
  const origin = record.origin === "user" ? "user" : "agent";
  const todo: Todo = { id, title, status, origin };
  for (const field of ["note", "summary", "verification", "commitSubject"] as const) {
    if (typeof record[field] === "string" && record[field] !== "") todo[field] = record[field] as string;
  }
  return todo;
}

function coerceGroup(raw: unknown): TodoGroup | null {
  if (typeof raw !== "object" || raw === null) return null;
  const record = raw as Record<string, unknown>;
  const id = typeof record.id === "string" ? record.id : "";
  const title = typeof record.title === "string" ? compress(record.title) : "";
  if (!id || !title || !Array.isArray(record.todos)) return null;
  const todos: Todo[] = [];
  for (const entry of record.todos) {
    const todo = coerceTodo(entry);
    if (todo) todos.push(todo);
  }
  return { id, title, todos };
}

function coercePlan(raw: unknown): TodoFile {
  if (typeof raw !== "object" || raw === null) return EMPTY_PLAN;
  const record = raw as Record<string, unknown>;
  if (record.v !== 1 || !Array.isArray(record.todos) || !Array.isArray(record.groups)) return EMPTY_PLAN;
  const todos: Todo[] = [];
  for (const entry of record.todos) {
    const todo = coerceTodo(entry);
    if (todo) todos.push(todo);
  }
  const groups: TodoGroup[] = [];
  for (const entry of record.groups) {
    const group = coerceGroup(entry);
    if (group) groups.push(group);
  }
  return { v: 1, todos, groups };
}

/**
 * Read one session's plan. A missing, unreadable, or wrong-shaped file reads
 * as an empty plan — a broken file must never take a session down, and the
 * next successful write replaces it wholesale.
 */
export function readPlan(sessionId: string, root: string): TodoFile {
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(planPath(sessionId, root), "utf8"));
  } catch {
    return EMPTY_PLAN;
  }
  return coercePlan(parsed);
}

/**
 * Write one session's plan: create the root, write a temp file beside the
 * target, rename. The rename is the atomic step — a reader either sees the
 * previous plan or the new one, never a truncated file.
 */
export function writePlan(sessionId: string, file: TodoFile, root: string): void {
  const path = planPath(sessionId, root);
  mkdirSync(root, { recursive: true });
  const temp = `${path}.tmp-${process.pid}-${Math.random().toString(36).slice(2, 8)}`;
  writeFileSync(temp, `${JSON.stringify(file, null, 2)}\n`, "utf8");
  renameSync(temp, path);
}

// --- operations ---------------------------------------------------------------

/** `\s+` → one space, both ends trimmed; a title is one line by definition. */
function compress(title: string): string {
  return title.replace(/\s+/g, " ").trim();
}

function todoById(file: TodoFile, id: string): { todo: Todo; group: TodoGroup | null } | null {
  for (const todo of file.todos) if (todo.id === id) return { todo, group: null };
  for (const group of file.groups) {
    for (const todo of group.todos) if (todo.id === id) return { todo, group };
  }
  return null;
}

/** A group's derived state: not stored, computed on every read. */
export function groupStatus(group: TodoGroup): "pending" | "active" | "done" {
  if (group.todos.length === 0) return "pending";
  if (group.todos.every((todo) => todo.status === "done")) return "done";
  if (group.todos.some((todo) => todo.status === "in_progress")) return "active";
  return "pending";
}

export interface AddTodoInput {
  title: string;
  /** An existing group id; without it the item joins the loose lane. */
  group?: string;
  /** `agent` by default; the panel passes `user` for its own adds. */
  origin?: TodoOrigin;
}

/** Adds one item, as a loose draft or as a group's next step. */
export function addTodo(file: TodoFile, input: AddTodoInput): TodoOpResult {
  const title = compress(typeof input.title === "string" ? input.title : "");
  if (!title) return { ok: false, reason: "empty-title" };
  const todo: Todo = { id: nextId(file, "t"), title, status: "pending", origin: input.origin ?? "agent" };
  if (input.group !== undefined) {
    const group = file.groups.find((candidate) => candidate.id === input.group);
    if (!group) return { ok: false, reason: "unknown-group" };
    return { ok: true, file: replaceGroup(file, group.id, { ...group, todos: [...group.todos, todo] }) };
  }
  return { ok: true, file: { ...file, todos: [...file.todos, todo] } };
}

export interface UpdateTodoInput {
  id: string;
  status?: TodoStatus;
  note?: string;
  summary?: string;
  verification?: string;
  commitSubject?: string;
}

/**
 * Patches one item. The plan keeps at most one `in_progress` across loose
 * lane and groups: setting one demotes the other. Leaving `done` drops the
 * completion fields unless the same patch provides them again (reopening a
 * step means its completion record is stale — a fresh run rewrites it).
 */
export function updateTodo(file: TodoFile, input: UpdateTodoInput): TodoOpResult {
  const found = todoById(file, input.id);
  if (!found) return { ok: false, reason: "unknown-id" };
  if (input.status !== undefined && !STATUSES.includes(input.status)) {
    return { ok: false, reason: "invalid-status" };
  }
  const todo = { ...found.todo };
  const wasDone = todo.status === "done";
  if (input.note !== undefined) {
    if (input.note === "") delete todo.note;
    else todo.note = input.note;
  }
  for (const field of ["summary", "verification", "commitSubject"] as const) {
    const value = input[field];
    if (value === undefined) continue;
    if (value === "") delete todo[field];
    else todo[field] = value;
  }
  if (input.status !== undefined) {
    todo.status = input.status;
    if (wasDone && input.status !== "done") {
      // Reopened: the completion record is stale unless this patch restates it.
      for (const field of ["summary", "verification", "commitSubject"] as const) {
        if (input[field] === undefined) delete todo[field];
      }
    }
  }
  let next = replaceTodo(file, todo.id, todo);
  if (todo.status === "in_progress") {
    next = demoteOtherInProgress(next, todo.id);
  }
  return { ok: true, file: next };
}

/** Sets every other `in_progress` item back to `pending` (the single-lane rule). */
function demoteOtherInProgress(file: TodoFile, keepId: string): TodoFile {
  const loose = file.todos.map((todo) =>
    todo.status === "in_progress" && todo.id !== keepId ? { ...todo, status: "pending" as const } : todo,
  );
  const groups = file.groups.map((group) => ({
    ...group,
    todos: group.todos.map((todo) =>
      todo.status === "in_progress" && todo.id !== keepId ? { ...todo, status: "pending" as const } : todo,
    ),
  }));
  return { ...file, todos: loose, groups };
}

function replaceTodo(file: TodoFile, id: string, todo: Todo): TodoFile {
  const loose = file.todos.map((candidate) => (candidate.id === id ? todo : candidate));
  const groups = file.groups.map((group) => {
    if (!group.todos.some((candidate) => candidate.id === id)) return group;
    return { ...group, todos: group.todos.map((candidate) => (candidate.id === id ? todo : candidate)) };
  });
  return { ...file, todos: loose, groups };
}

function replaceGroup(file: TodoFile, id: string, group: TodoGroup): TodoFile {
  return { ...file, groups: file.groups.map((candidate) => (candidate.id === id ? group : candidate)) };
}

/**
 * The only path from the loose lane into a group: the draft becomes the
 * first step of a new group titled after it, keeping its id (a promoted
 * draft is the same item, now with a home). An empty loose lane afterwards
 * is fine; a group may also hold zero steps elsewhere in the file.
 */
export function promoteTodo(file: TodoFile, id: string): TodoOpResult {
  const found = todoById(file, id);
  if (!found) return { ok: false, reason: "unknown-id" };
  if (found.group) return { ok: false, reason: "grouped-step" };
  const todo = found.todo;
  const title = compress(todo.title);
  if (!title) return { ok: false, reason: "empty-title" };
  if (file.groups.some((group) => group.title === title)) return { ok: false, reason: "title-collision" };
  const group: TodoGroup = { id: nextId(file, "g"), title, todos: [todo] };
  return {
    ok: true,
    file: {
      ...file,
      todos: file.todos.filter((candidate) => candidate.id !== id),
      groups: [...file.groups, group],
    },
  };
}

/** Removes exactly one item; a group it empties is removed with it. */
export function removeTodo(file: TodoFile, id: string): TodoOpResult {
  if (!todoById(file, id)) return { ok: false, reason: "unknown-id" };
  const todos = file.todos.filter((candidate) => candidate.id !== id);
  const groups: TodoGroup[] = [];
  for (const group of file.groups) {
    const had = group.todos.some((candidate) => candidate.id === id);
    const remaining = group.todos.filter((candidate) => candidate.id !== id);
    // Only a group this removal actually emptied goes away; a deliberately
    // empty group stays when an unrelated item is removed elsewhere.
    if (had && remaining.length === 0) continue;
    groups.push({ ...group, todos: remaining });
  }
  return { ok: true, file: { ...file, todos, groups } };
}

export interface WriteTodoInput {
  id?: string;
  title: string;
}

export interface WriteGroupInput {
  /** An existing group id renames that group; without one a group is created. */
  id?: string;
  title: string;
  todos?: WriteTodoInput[];
}

export interface WriteGroupsInput {
  groups: WriteGroupInput[];
}

/**
 * Reconcile the groups with the given structure (the `todo_write` core):
 *
 * - a known group id keeps its items' statuses (a status in the input is
 *   ignored — the model cannot flip steps by re-listing them) and may be
 *   renamed;
 * - an unknown or absent group id creates a group;
 * - an item id already in that group keeps the item as it is, position per
 *   the input; an item id from another group moves in (group↔group is a
 *   restructure, not a promotion);
 * - an item id from the loose lane is refused (`loose-item`): promote is the
 *   only lane→group path, and the loose lane is never this tool's target;
 * - an unseen item id becomes a new pending step; an absent id makes one up;
 * - grouped item ids missing from the input are removed — listing a group's
 *   steps is how a completed, superseded step leaves the plan;
 * - the loose lane is untouched by all of this: nothing there is created,
 *   moved, renamed, or deleted by `todo_write`;
 * - group titles stay unique after compression; duplicates refuse the op.
 */
export function writeGroups(file: TodoFile, input: WriteGroupsInput): TodoOpResult {
  if (!Array.isArray(input.groups)) return { ok: false, reason: "invalid-op" };
  // The file's whole id universe; generated and client-supplied ids must
  // steer clear of it and of each other for the final file to stay unique.
  const universe = new Set<string>();
  const itemById = new Map<string, Todo>();
  const looseIds = new Set(file.todos.map((todo) => todo.id));
  const existingGroups = new Map<string, TodoGroup>();
  for (const todo of file.todos) {
    universe.add(todo.id);
    itemById.set(todo.id, todo);
  }
  for (const group of file.groups) {
    universe.add(group.id);
    existingGroups.set(group.id, group);
    for (const todo of group.todos) {
      universe.add(todo.id);
      itemById.set(todo.id, todo);
    }
  }

  const claimed = new Set<string>(); // file item ids the input keeps or moves
  const minted = new Set<string>(); // item ids created by this input
  const mintedGroups = new Set<string>();
  const freshId = (prefix: string): string => {
    let id = nextId(file, prefix);
    while (universe.has(id) || claimed.has(id) || minted.has(id) || mintedGroups.has(id)) {
      id = `${prefix}-${(Number.parseInt(id.slice(prefix.length + 1), 36) + 1).toString(36)}`;
    }
    return id;
  };

  const groups: TodoGroup[] = [];
  for (const spec of input.groups) {
    const title = compress(typeof spec.title === "string" ? spec.title : "");
    if (!title) return { ok: false, reason: "empty-title" };

    let groupId: string;
    if (spec.id !== undefined) {
      if (!GROUP_ID_PATTERN.test(spec.id)) return { ok: false, reason: "invalid-id" };
      if (!existingGroups.has(spec.id)) return { ok: false, reason: "unknown-group" };
      if (groups.some((group) => group.id === spec.id)) return { ok: false, reason: "invalid-id" };
      groupId = spec.id;
    } else {
      groupId = freshId("g");
      mintedGroups.add(groupId);
    }

    const steps: Todo[] = [];
    for (const entry of spec.todos ?? []) {
      const stepTitle = compress(typeof entry.title === "string" ? entry.title : "");
      if (!stepTitle) return { ok: false, reason: "empty-title" };
      if (entry.id !== undefined && itemById.has(entry.id)) {
        if (looseIds.has(entry.id)) return { ok: false, reason: "loose-item" };
        if (claimed.has(entry.id)) return { ok: false, reason: "invalid-id" };
        claimed.add(entry.id);
        steps.push(itemById.get(entry.id)!); // status, note, completion all preserved
      } else if (entry.id !== undefined) {
        // A group id is in `universe` but not an item: never coerce it into a
        // step (that used to write `null` into the file and break the tools).
        if (universe.has(entry.id)) return { ok: false, reason: "invalid-id" };
        if (!ID_PATTERN.test(entry.id) || minted.has(entry.id)) return { ok: false, reason: "invalid-id" };
        minted.add(entry.id);
        steps.push({ id: entry.id, title: stepTitle, status: "pending", origin: "agent" });
      } else {
        const id = freshId("t");
        minted.add(id);
        steps.push({ id, title: stepTitle, status: "pending", origin: "agent" });
      }
    }
    groups.push({ id: groupId, title, todos: steps });
  }

  // Group titles stay unique in the resulting file: every group the input
  // omits is dropped (the groups array is rebuilt from the input), so the old
  // titles below are only there to catch a stale id-less replacement. An
  // id-less group whose title matches an existing, still-mentioned group is
  // refused: the model must pass that group's id instead of silently
  // replacing (and emptying) it.
  const finalTitles = new Map<string, string>();
  for (const group of groups) finalTitles.set(group.id, group.title);
  for (const existing of file.groups) {
    if (!finalTitles.has(existing.id)) finalTitles.set(existing.id, existing.title);
  }
  const seenTitles = new Set<string>();
  for (const title of finalTitles.values()) {
    if (seenTitles.has(title)) return { ok: false, reason: "title-collision" };
    seenTitles.add(title);
  }

  // Grouped item ids absent from the input are simply gone: the groups array
  // is rebuilt from the input alone. The loose lane is passed through as is.
  return { ok: true, file: { ...file, groups } };
}

// --- op dispatch (the `_zu/todos/write` envelope) -----------------------------

/** The one op envelope the host channel accepts; the tools build their own calls. */
export type TodoOp =
  | { kind: "add"; title: string; group?: string; origin?: TodoOrigin }
  | { kind: "update"; id: string; status?: TodoStatus; note?: string; summary?: string; verification?: string; commitSubject?: string }
  | { kind: "promote"; id: string }
  | { kind: "remove"; id: string }
  | { kind: "writeGroups"; groups: WriteGroupInput[] };

/**
 * Applies one op to the session's file on disk and writes the result back.
 * Read-op-write is one synchronous stretch, so a panel edit and a tool call
 * on the same process never interleave mid-operation.
 */
export function applyTodoOp(sessionId: string, root: string, op: unknown): TodoOpResult {
  if (typeof op !== "object" || op === null) return { ok: false, reason: "invalid-op" };
  const record = op as Record<string, unknown>;
  const file = readPlan(sessionId, root);
  let result: TodoOpResult;
  switch (record.kind) {
    case "add":
      result = addTodo(file, {
        title: typeof record.title === "string" ? record.title : "",
        ...(typeof record.group === "string" ? { group: record.group } : {}),
        ...(record.origin === "user" ? { origin: "user" as const } : {}),
      });
      break;
    case "update":
      result = updateTodo(file, {
        id: typeof record.id === "string" ? record.id : "",
        ...(isStatus(record.status) ? { status: record.status } : {}),
        ...(typeof record.note === "string" ? { note: record.note } : {}),
        ...(typeof record.summary === "string" ? { summary: record.summary } : {}),
        ...(typeof record.verification === "string" ? { verification: record.verification } : {}),
        ...(typeof record.commitSubject === "string" ? { commitSubject: record.commitSubject } : {}),
      });
      break;
    case "promote":
      result = promoteTodo(file, typeof record.id === "string" ? record.id : "");
      break;
    case "remove":
      result = removeTodo(file, typeof record.id === "string" ? record.id : "");
      break;
    case "writeGroups":
      result = writeGroups(file, { groups: record.groups as WriteGroupInput[] });
      break;
    default:
      return { ok: false, reason: "invalid-op" };
  }
  if (result.ok) writePlan(sessionId, result.file, root);
  return result;
}

function isStatus(value: unknown): value is TodoStatus {
  return typeof value === "string" && STATUSES.includes(value as TodoStatus);
}

// --- rendering (tool output) ---------------------------------------------------

/** One item's compact line: `[x] t-3 Title` with the state in brackets. */
function renderTodo(todo: Todo, indent: string): string {
  const mark = todo.status === "done" ? "x" : todo.status === "in_progress" ? "~" : " ";
  const suffix =
    todo.status === "done"
      ? todo.verification
        ? ` — verified: ${todo.verification}`
        : ""
      : todo.note
        ? ` — ${todo.note}`
        : "";
  return `${indent}[${mark}] ${todo.id} ${todo.title}${suffix}`;
}

/** The plan as the model reads it: loose drafts first, then each group. */
export function formatPlan(file: TodoFile): string {
  const lines: string[] = [];
  if (file.todos.length > 0) {
    lines.push("loose drafts:");
    for (const todo of file.todos) lines.push(renderTodo(todo, "  "));
  }
  for (const group of file.groups) {
    lines.push(`group ${group.id} "${group.title}" — ${groupStatus(group)}`);
    for (const todo of group.todos) lines.push(renderTodo(todo, "  "));
  }
  if (lines.length === 0) return "the plan is empty";
  return lines.join("\n");
}
