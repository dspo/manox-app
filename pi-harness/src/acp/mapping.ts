/**
 * pi vocabulary → ACP vocabulary, as pure functions.
 *
 * These are the v2 §5.2 core mappings: prompt content blocks into pi prompt
 * text, streaming deltas into `agent_message_chunk` / `agent_thought_chunk`,
 * tool executions into `tool_call` / `tool_call_update`, and pi's message
 * stop reasons into ACP stop reasons. The module is dependency-free beyond
 * ACP types, so `bun test` exercises it without pi and without network.
 *
 * Deliberately absent here (later P stages, per docs/pi-acp-PROTOCOL.md):
 * thinking levels as session modes, model selection, compaction state events,
 * usage updates, permission requests, `_session/steering`.
 */

import type { ContentBlock, SessionUpdate, StopReason, ToolKind } from "@agentclientprotocol/sdk";
import { JSONRPC_CODE, codedError, type RequestError } from "./errors";

/**
 * The `_meta.pi` payload the harness attaches to its transcript chunks
 * (issue #20 线协议增量): timestamps, the producing model, and the settle
 * facts of a message entry. It rides ACP's reserved `_meta`, so the schema
 * and the protocol version are untouched.
 */
export interface PiMeta {
  /** Wall-clock milliseconds the message was written. */
  at?: number;
  /** The model that produced the message. */
  model?: { provider: string; id: string; name?: string };
  /** The pi session entry this chunk belongs to (stable across replay). */
  entryId?: string;
  /** Settle marker: the message entry landed in the session file. */
  settled?: boolean;
  /** Milliseconds from the assistant's first delta to its settled entry. */
  durationMs?: number;
  /** pi's message stop reason (ACP StopReason vocabulary). */
  stopReason?: StopReason | string;
  /** pi's provider error message when the turn ended in one. */
  error?: string;
}

/**
 * The `_meta.zu` payload on a tool call's **end** frame (issue #20 §5.2; the
 * namespace #8's structured `details` will share). Only the facts a tool
 * card renders — never the full `details` (pi does not persist them anyway,
 * so a replay carries none of this).
 */
export interface ZuToolMeta {
  /** Milliseconds `execute()` took (pi's monotonic clock). */
  durationMs?: number;
  /** The bash tool's exit code, when the result carried a numeric one. */
  exitCode?: number;
  /** The output was truncated by pi's output accumulator. */
  truncated?: boolean;
  /** The full output's line count, when truncated. */
  totalLines?: number;
  /** Temp file holding the full output, when truncated. */
  fullOutputPath?: string;
}

/**
 * ContentBlock[] → the prompt text pi's `session.prompt` takes.
 *
 * ACP's baseline requires text and `resource_link`; a `resource_link` is a
 * pointer the client resolved for the user, so it goes into the prompt as a
 * markdown link. Everything else is a declared no (the initialize reply says
 * so through `promptCapabilities`), refused with a coded invalid-params error
 * rather than silently dropped.
 */
export function promptBlocksToText(
  blocks: ContentBlock[],
): { ok: true; text: string } | { ok: false; error: RequestError } {
  if (!Array.isArray(blocks) || blocks.length === 0) {
    return {
      ok: false,
      error: codedError(JSONRPC_CODE.invalidParams, "empty_prompt", "session/prompt needs at least one content block"),
    };
  }
  let text = "";
  for (const block of blocks) {
    switch (block?.type) {
      case "text":
        text += block.text;
        break;
      case "resource_link":
        text += `[${block.name ?? block.uri}](${block.uri})`;
        break;
      case "image":
        return {
          ok: false,
          error: codedError(
            JSONRPC_CODE.invalidParams,
            "image_not_supported",
            "this harness does not accept image content blocks (promptCapabilities.image is false)",
          ),
        };
      case "audio":
        return {
          ok: false,
          error: codedError(
            JSONRPC_CODE.invalidParams,
            "audio_not_supported",
            "this harness does not accept audio content blocks (promptCapabilities.audio is false)",
          ),
        };
      case "resource":
        return {
          ok: false,
          error: codedError(
            JSONRPC_CODE.invalidParams,
            "embedded_context_not_supported",
            "this harness does not accept embedded resource blocks (promptCapabilities.embeddedContext is false)",
          ),
        };
      default:
        return {
          ok: false,
          error: codedError(
            JSONRPC_CODE.invalidParams,
            "content_block_not_supported",
            `unsupported content block type: ${String((block as { type?: unknown })?.type)}`,
          ),
        };
    }
  }
  if (text.trim().length === 0) {
    return {
      ok: false,
      error: codedError(JSONRPC_CODE.invalidParams, "empty_prompt", "session/prompt carries no text"),
    };
  }
  return { ok: true, text };
}

/**
 * pi tool names → ACP tool kinds. The host renders the generic card
 * (title/kind/status/locations) and never specializes on pi tool names
 * (v2 §2.5/§5.2).
 */
export function piToolKind(toolName: string): ToolKind {
  const name = toolName.toLowerCase();
  if (["read", "ls", "find", "view"].includes(name)) return "read";
  if (["grep", "glob", "search"].includes(name)) return "search";
  if (["edit", "write", "multiedit", "patch", "apply_patch"].includes(name)) return "edit";
  if (["rm", "delete", "remove"].includes(name)) return "delete";
  if (["mv", "move"].includes(name)) return "move";
  if (["bash", "execute", "run"].includes(name)) return "execute";
  if (["think", "plan", "thinking"].includes(name)) return "think";
  if (["fetch", "webfetch", "web_search", "browser"].includes(name)) return "fetch";
  return "other";
}

/** The arg keys a card title may quote, most salient first. */
const TITLE_ARG_KEYS = ["path", "file_path", "command", "pattern", "url", "query", "name", "description"];

/** A short human title for a tool card: the tool plus its salient argument. */
export function describeToolCall(toolName: string, args: unknown): string {
  if (!isRecord(args)) return toolName;
  for (const key of TITLE_ARG_KEYS) {
    const value = args[key];
    if (typeof value === "string" && value.trim().length > 0) {
      return `${toolName} ${truncate(value.trim())}`;
    }
  }
  return toolName;
}

/** `tool_execution_start` → a `tool_call` update (the card opens). */
export function toolCallStartUpdate(toolCallId: string, toolName: string, args: unknown): SessionUpdate {
  const locations =
    isRecord(args) && typeof args.path === "string" && args.path.length > 0 ? [{ path: args.path }] : undefined;
  return {
    sessionUpdate: "tool_call",
    toolCallId,
    title: describeToolCall(toolName, args),
    name: toolName,
    kind: piToolKind(toolName),
    status: "in_progress",
    ...(isRecord(args) ? { rawInput: args } : {}),
    ...(locations ? { locations } : {}),
  };
}

/**
 * `tool_execution_update` → a `tool_call_update` with the partial output as
 * text when it has a textual shape; anything else is left out rather than
 * stringified blindly (the final update carries the result).
 */
export function toolCallPartialUpdate(toolCallId: string, partialResult: unknown): SessionUpdate | undefined {
  const text = partialText(partialResult);
  if (text === undefined) return undefined;
  return {
    sessionUpdate: "tool_call_update",
    toolCallId,
    status: "in_progress",
    content: [{ type: "content", content: { type: "text", text } }],
  };
}

/** The largest `details` payload the harness forwards: a bigger one costs
 * more than a card can carry, so it drops whole (the text still flows). */
const DETAILS_LIMIT_BYTES = 64 * 1024;

/**
 * The tool's own details, when they are a bounded plain JSON value.
 *
 * pi tool results are `{ content, details }` and `details` is the validated
 * shape card renderers read — a contract, not UI-private parsing, so it is
 * forwarded as-is. Anything that is not a plain object/array, or anything
 * past the byte limit or not serializable, drops whole: no truncation into
 * half a JSON, no guessing at a shape.
 */
export function toolDetails(result: unknown): unknown | undefined {
  if (!isRecord(result)) return undefined;
  const details = (result as { details?: unknown }).details;
  if (details === undefined || details === null) return undefined;
  if (typeof details !== "object") return undefined;
  try {
    if (Buffer.byteLength(JSON.stringify(details), "utf8") > DETAILS_LIMIT_BYTES) return undefined;
  } catch {
    return undefined; // circular / non-serializable
  }
  return details;
}

/**
 * `tool_execution_end` → the card settles: completed or failed. An edit's
 * arguments ride again as `ToolCallContent.diff` blocks, one per replacement,
 * so the client renders what changed without a file read; the card's render
 * facts (`durationMs`, `exitCode`, truncation) and the structured `details`
 * ride one `_meta.zu` — end frames only, never the partial ones.
 */
export function toolCallEndUpdate(
  toolCallId: string,
  toolName: string,
  result: unknown,
  isError: boolean,
  args?: unknown,
  stats?: ZuToolMeta,
): SessionUpdate {
  type Content =
    | ReturnType<typeof toolResultContent>[number]
    | ReturnType<typeof editDiffBlocks>[number];
  const content: Content[] = toolResultContent(result);
  for (const diff of editDiffBlocks(toolName, args)) content.push(diff);
  const statsMeta = cleanZuMeta(stats);
  const zu: Record<string, unknown> = { ...(statsMeta ?? {}) };
  const details = toolDetails(result);
  if (details !== undefined) zu.details = details;
  return {
    sessionUpdate: "tool_call_update",
    toolCallId,
    status: isError ? "failed" : "completed",
    ...(content.length > 0 ? { content } : {}),
    ...(Object.keys(zu).length > 0 ? { _meta: { zu } } : {}),
  };
}

/** The edit-like tools whose arguments describe exact replacements. */
const EDIT_TOOLS = new Set(["edit", "write", "multiedit", "patch", "apply_patch"]);

/** A `Diff` content block per replacement op in the tool's arguments. */
export function editDiffBlocks(
  toolName: string,
  args: unknown,
): Array<{ type: "diff"; path: string; oldText?: string | null; newText: string }> {
  if (!args || !isRecord(args) || typeof args.path !== "string" || !EDIT_TOOLS.has(toolName.toLowerCase())) {
    return [];
  }
  const path = args.path;
  const ops = Array.isArray(args.edits)
    ? args.edits.filter(isRecord)
    : isRecord(args) && typeof args.newText === "string"
      ? [args]
      : [];
  const blocks: Array<{ type: "diff"; path: string; oldText?: string | null; newText: string }> = [];
  for (const op of ops) {
    if (typeof op.newText !== "string") continue;
    blocks.push({
      type: "diff",
      path,
      ...(typeof op.oldText === "string" ? { oldText: op.oldText } : { oldText: null }),
      newText: op.newText,
    });
  }
  return blocks;
}

/**
 * The card's `_meta.zu` from a `tool_execution_end`: the event's monotonic
 * `durationMs`, the bash tool's numeric `exit_code` (structuredContent),
 * and the output accumulator's truncation facts (`details`). Absent facts
 * are dropped — the client renders only what it received.
 */
export function toolStats(durationMs: unknown, result: unknown): ZuToolMeta {
  const zu: ZuToolMeta = {};
  if (typeof durationMs === "number" && Number.isFinite(durationMs) && durationMs >= 0) {
    zu.durationMs = durationMs;
  }
  if (isRecord(result)) {
    const structured = result.structuredContent;
    if (isRecord(structured) && typeof structured.exit_code === "number" && Number.isFinite(structured.exit_code)) {
      zu.exitCode = structured.exit_code;
    }
    const details = result.details;
    if (isRecord(details)) {
      const truncation = details.truncation;
      if (isRecord(truncation)) {
        if (typeof truncation.truncated === "boolean") zu.truncated = truncation.truncated;
        if (typeof truncation.totalLines === "number" && Number.isFinite(truncation.totalLines)) {
          zu.totalLines = truncation.totalLines;
        }
      }
      if (typeof details.fullOutputPath === "string" && details.fullOutputPath.length > 0) {
        zu.fullOutputPath = details.fullOutputPath;
      }
    }
  }
  return zu;
}

/** Drops the absent fields, so an all-optional meta never ships as `{}`. */
function cleanZuMeta(stats?: ZuToolMeta): ZuToolMeta | undefined {
  if (!stats) return undefined;
  const zu: ZuToolMeta = {};
  for (const [key, value] of Object.entries(stats)) {
    if (value !== undefined) (zu as Record<string, unknown>)[key] = value;
  }
  return Object.keys(zu).length > 0 ? zu : undefined;
}

/**
 * A streaming assistant delta → its chunk update. Only text and thinking
 * deltas are protocol chunks; tool-call argument deltas are transport detail
 * (the tool card comes from the execution events) and yield `undefined`.
 * `meta` rides `_meta.pi` so the client can anchor the message (timestamp,
 * model) without a side channel.
 */
export function assistantDeltaUpdate(
  assistantMessageEvent: unknown,
  messageId: string,
  meta?: PiMeta,
): SessionUpdate | undefined {
  if (!isRecord(assistantMessageEvent)) return undefined;
  const { type, delta } = assistantMessageEvent;
  if (typeof delta !== "string" || delta.length === 0) return undefined;
  const pi = cleanMeta(meta);
  if (type === "text_delta") {
    return {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: delta },
      messageId,
      ...(pi ? { _meta: { pi } } : {}),
    };
  }
  if (type === "thinking_delta") {
    return {
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: delta },
      messageId,
      ...(pi ? { _meta: { pi } } : {}),
    };
  }
  return undefined;
}

/**
 * The settle marker (issue #20 §5.1): an empty-text `agent_message_chunk`
 * whose `_meta.pi` says the assistant's message entry landed in the session
 * file — its id, timing, stop reason and error. A client ignores the empty
 * text and reads `_meta`; a strict validator still sees a legal chunk.
 */
export function settledUpdate(meta: PiMeta & { entryId: string }): SessionUpdate {
  const pi: PiMeta = { settled: true, ...cleanMeta(meta) };
  return {
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text: "" },
    messageId: meta.entryId,
    _meta: { pi },
  };
}

/** Drops the absent fields, so an all-optional meta never ships as `{}`. */
function cleanMeta(meta?: PiMeta): PiMeta | undefined {
  if (!meta) return undefined;
  const pi: PiMeta = {};
  for (const [key, value] of Object.entries(meta)) {
    if (value !== undefined) (pi as Record<string, unknown>)[key] = value;
  }
  return Object.keys(pi).length > 0 ? pi : undefined;
}

/**
 * A user message entry appended to the session file → its chunks. This is
 * the live twin of the replay's `user_message_chunk` (same messageId, same
 * `_meta.pi.at`), so the client's optimistic row re-keys onto the entry id
 * through one code path. The harness accepts text prompts only, so the
 * entry's content is text; anything else stays unshipped.
 */
export function userEntryUpdates(entryId: string, content: unknown, at: number): SessionUpdate[] {
  const updates: SessionUpdate[] = [];
  const text = userEntryText(content);
  if (text) {
    updates.push({
      sessionUpdate: "user_message_chunk",
      content: { type: "text", text },
      messageId: entryId,
      _meta: { pi: { at, entryId } },
    });
  }
  return updates;
}

/** The displayable text of a user message entry's content. */
export function userEntryText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const block of content) {
    if (isRecord(block) && block.type === "text" && typeof block.text === "string") parts.push(block.text);
  }
  return parts.join("");
}

/**
 * pi's message stop reason → the `session/prompt` stopReason.
 *
 * `error` maps to `end_turn` because ACP stop reasons have no error case;
 * the error text itself rides the settle marker's `_meta.pi` (`stopReason`,
 * `error`), so the client still sees why the turn ended. `toolUse` never
 * ends a settled turn; `deferred` (pi continues asynchronously) is reported
 * as `end_turn` until a P stage gives it a real shape.
 */
export function stopReasonFromPi(piStopReason: string | undefined): StopReason {
  switch (piStopReason) {
    case "aborted":
      return "cancelled";
    case "length":
      return "max_tokens";
    default:
      return "end_turn";
  }
}

/** Extracts displayable text from a pi tool result object. */
export function toolResultContent(result: unknown): Array<{ type: "content"; content: { type: "text"; text: string } }> {
  if (!isRecord(result) || !Array.isArray(result.content)) return [];
  const content: Array<{ type: "content"; content: { type: "text"; text: string } }> = [];
  for (const block of result.content) {
    if (isRecord(block) && block.type === "text" && typeof block.text === "string" && block.text.length > 0) {
      content.push({ type: "content", content: { type: "text", text: block.text } });
    }
    // Image results are dropped here: the harness declares no image support
    // on the prompt side, and a tool card image would render unexplained.
  }
  return content;
}

/** Text from a partial tool update when it carries any. */
function partialText(partialResult: unknown): string | undefined {
  if (typeof partialResult === "string" && partialResult.length > 0) return partialResult;
  if (isRecord(partialResult)) {
    if (typeof partialResult.text === "string" && partialResult.text.length > 0) return partialResult.text;
    // pi tool streams expose partial output as content blocks too.
    if (Array.isArray(partialResult.content)) {
      const texts = partialResult.content
        .filter((block): block is { type: "text"; text: string } => isRecord(block) && block.type === "text")
        .map((block) => block.text)
        .filter((text) => typeof text === "string" && text.length > 0);
      if (texts.length > 0) return texts.join("");
    }
  }
  return undefined;
}

function truncate(value: string, max = 80): string {
  const line = value.split("\n")[0] ?? value;
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
