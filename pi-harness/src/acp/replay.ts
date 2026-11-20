/**
 * session/load history replay: pi session data → `session/update` stream.
 *
 * ACP v1 semantics (v2 §4.3): a `session/load` MUST replay the conversation
 * as `session/update` notifications before the load response returns — the
 * updates are how a client that knows nothing about pi files rebuilds its
 * view. The input is pi's own session projection (`SessionManager.
 * buildSessionProjection()`): the compaction-aware, leaf-branch-resolved
 * message list with the provenance of each message's source entry, so the
 * replay shows what the model actually sees and `messageId`s stay stable.
 *
 * Every chunk carries `_meta.pi` (`at`, `entryId`; assistant text also the
 * model and the settle facts), so the client's transcript reducer builds the
 * same entries from a replay as from a live turn. An assistant entry closes
 * with the settle marker exactly as a live turn does.
 *
 * Parts of pi session data with no ACP chunk shape are skipped here, on
 * purpose and registered (v2: "pi 会话数据里映射不了的部件跳过并在代码注释登记"):
 *
 *   - `system` messages — pi's own prompt lines, not conversation.
 *   - image blocks inside replayed user messages — the harness declares no
 *     image support (promptCapabilities.image false); replay stays text-only
 *     until a P stage declares images end to end.
 *   - empty or redacted thinking blocks — nothing displayable.
 *   - tool calls whose result never arrived (interrupted turns) — replayed
 *     as `tool_call` left `in_progress`, which is the honest state; the next
 *     `session/load` after a resumed turn shows the settled pair.
 *   - state-only entries (model_change, thinking_level_change, usage, zu.*
 *     custom entries, labels) — pi's projection already drops them from the
 *     message list; they are agent-side state, not conversation.
 *
 * Tool statistics (`_meta.zu`: durationMs/exitCode/truncation) are
 * deliberately absent from a replay: pi does not persist tool `details`, so
 * there is nothing honest to send — the client renders the card without the
 * stats (a registered known gap, issue #20 §5.4).
 *
 * Compaction and branch summaries DO replay: pi's projection resolves them
 * into the summary messages that now stand in the context.
 */

import type { SessionUpdate } from "@agentclientprotocol/sdk";
import {
  describeToolCall,
  editDiffBlocks,
  isRecord,
  piToolKind,
  settledUpdate,
  stopReasonFromPi,
  type PiMeta,
} from "./mapping";

/** The one projection entry this module consumes. */
export interface ReplayEntry {
  /** The pi session entry's id — becomes the chunk `messageId`. */
  sourceId: string;
  /** The pi session entry's write time (ISO); anchors the chunk `_meta.pi.at`. */
  timestamp: string;
  /** The messages pi's projection resolved this entry into. */
  messages: ReplayMessage[];
}

/** The subset of a pi `AgentMessage` the replay reads. */
export interface ReplayMessage {
  role: string;
  content: unknown;
  toolCallId?: unknown;
  toolName?: unknown;
  isError?: unknown;
  /** Wall-clock milliseconds (pi stamps its messages). */
  timestamp?: unknown;
  /** The assistant message's model facts (`provider`, `model`, `responseModel`). */
  provider?: unknown;
  model?: unknown;
  responseModel?: unknown;
  /** The assistant message's settle facts. */
  stopReason?: unknown;
  errorMessage?: unknown;
}

/** Maps a pi session projection onto the replay update list, in order. */
export function replayUpdates(entries: ReplayEntry[]): SessionUpdate[] {
  const updates: SessionUpdate[] = [];
  // Edit arguments met along the way: their tool result replays with the
  // same `ToolCallContent.diff` blocks a live turn sends.
  const argsByToolCall = new Map<string, { name: string; args: unknown }>();
  for (const entry of entries) {
    for (const message of entry.messages) {
      pushMessage(updates, entry, message, argsByToolCall);
    }
  }
  return updates;
}

function pushMessage(
  updates: SessionUpdate[],
  entry: ReplayEntry,
  message: ReplayMessage,
  argsByToolCall: Map<string, { name: string; args: unknown }>,
): void {
  switch (message.role) {
    case "user":
      pushUserContent(updates, entry, message);
      return;
    case "assistant":
      pushAssistantContent(updates, entry, message, argsByToolCall);
      return;
    case "toolResult":
      pushToolResult(updates, message, argsByToolCall);
      return;
    default:
      // `system` and anything newer: no ACP shape (see header comment).
      return;
  }
}

function pushUserContent(updates: SessionUpdate[], entry: ReplayEntry, message: ReplayMessage): void {
  const at = metaAt(entry, message);
  for (const text of userText(message.content)) {
    if (text.length > 0) {
      updates.push({
        sessionUpdate: "user_message_chunk",
        content: { type: "text", text },
        messageId: entry.sourceId,
        _meta: { pi: { at, entryId: entry.sourceId } },
      });
    }
  }
}

function pushAssistantContent(
  updates: SessionUpdate[],
  entry: ReplayEntry,
  message: ReplayMessage,
  argsByToolCall: Map<string, { name: string; args: unknown }>,
): void {
  if (!Array.isArray(message.content)) return;
  const anchor: PiMeta = assistantMeta(entry, message);
  for (const block of message.content) {
    if (!isRecord(block)) continue;
    if (block.type === "text" && typeof block.text === "string" && block.text.length > 0) {
      updates.push({
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: block.text },
        messageId: entry.sourceId,
        _meta: { pi: { at: anchor.at, entryId: anchor.entryId, ...(anchor.model ? { model: anchor.model } : {}) } },
      });
    } else if (block.type === "thinking" && typeof block.thinking === "string" && block.thinking.length > 0) {
      updates.push({
        sessionUpdate: "agent_thought_chunk",
        content: { type: "text", text: block.thinking },
        messageId: entry.sourceId,
        _meta: { pi: { at: anchor.at, entryId: anchor.entryId } },
      });
    } else if (block.type === "toolCall" && typeof block.id === "string" && typeof block.name === "string") {
      argsByToolCall.set(block.id, { name: block.name, args: block.arguments });
      updates.push({
        sessionUpdate: "tool_call",
        toolCallId: block.id,
        title: describeToolCall(block.name, block.arguments),
        name: block.name,
        kind: piToolKind(block.name),
        status: "in_progress",
        ...(isRecord(block.arguments) ? { rawInput: block.arguments } : {}),
      });
    }
    // image blocks: skipped (header comment).
  }
  // The settle marker closes the entry exactly as a live turn does, so the
  // client's reducer takes one path; an error entry with no text still lands.
  updates.push(settledUpdate({ settled: true, entryId: entry.sourceId, ...anchor }));
}

/** The `_meta.pi.at`: the message's own stamp when it has one, else the entry's. */
function metaAt(entry: ReplayEntry, message: ReplayMessage): number {
  if (typeof message.timestamp === "number" && Number.isFinite(message.timestamp)) return message.timestamp;
  const parsed = Date.parse(entry.timestamp);
  return Number.isFinite(parsed) ? parsed : 0;
}

/** A replayed assistant entry's meta: anchored and settled by construction. */
function assistantMeta(entry: ReplayEntry, message: ReplayMessage): PiMeta {
  const model =
    typeof message.provider === "string" && typeof message.model === "string"
      ? {
          provider: message.provider,
          id: message.model,
          ...(typeof message.responseModel === "string" ? { name: message.responseModel } : {}),
        }
      : undefined;
  return {
    at: metaAt(entry, message),
    entryId: entry.sourceId,
    ...(model ? { model } : {}),
    ...(typeof message.stopReason === "string" ? { stopReason: stopReasonFromPi(message.stopReason) } : {}),
    ...(typeof message.errorMessage === "string" && message.errorMessage ? { error: message.errorMessage } : {}),
  };
}

function pushToolResult(
  updates: SessionUpdate[],
  message: ReplayMessage,
  argsByToolCall: Map<string, { name: string; args: unknown }>,
): void {
  if (typeof message.toolCallId !== "string") return;
  type Content =
    | { type: "content"; content: { type: "text"; text: string } }
    | { type: "diff"; path: string; oldText?: string | null; newText: string };
  const content: Content[] = [];
  if (Array.isArray(message.content)) {
    for (const block of message.content) {
      if (isRecord(block) && block.type === "text" && typeof block.text === "string" && block.text.length > 0) {
        content.push({ type: "content", content: { type: "text", text: block.text } });
      }
    }
  }
  const remembered = argsByToolCall.get(message.toolCallId);
  if (remembered) {
    argsByToolCall.delete(message.toolCallId);
    for (const diff of editDiffBlocks(remembered.name, remembered.args)) content.push(diff);
  }
  updates.push({
    sessionUpdate: "tool_call_update",
    toolCallId: message.toolCallId,
    status: message.isError === true ? "failed" : "completed",
    ...(content.length > 0 ? { content } : {}),
  });
}

/** User content is a string or a block list; images and unknown blocks drop. */
function userText(content: unknown): string[] {
  if (typeof content === "string") return [content];
  if (!Array.isArray(content)) return [];
  const texts: string[] = [];
  for (const block of content) {
    if (isRecord(block) && block.type === "text" && typeof block.text === "string") texts.push(block.text);
  }
  return texts;
}
