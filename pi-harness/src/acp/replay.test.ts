/**
 * Unit tests for the session/load replay: pi projection messages in,
 * `session/update` payloads out, with the skips registered in replay.ts.
 */

import { describe, expect, test } from "bun:test";
import { replayUpdates, type ReplayEntry } from "./replay";

const entry = (sourceId: string, messages: ReplayEntry["messages"]): ReplayEntry => ({
  sourceId,
  timestamp: "2026-11-16T08:00:00.000Z",
  messages,
});

describe("replayUpdates", () => {
  test("replays user, assistant text, thinking and tool pairs in order", () => {
    const updates = replayUpdates([
      entry("e1", [{ role: "user", content: "fix the bug" }]),
      entry("e2", [
        {
          role: "assistant",
          content: [
            { type: "thinking", thinking: "look at the parser" },
            { type: "text", text: "On it." },
            { type: "toolCall", id: "tc1", name: "read", arguments: { path: "/a.ts" } },
          ],
          provider: "dashscope",
          model: "qwen3.7-max",
          stopReason: "stop",
        },
      ]),
      entry("e3", [
        { role: "toolResult", toolCallId: "tc1", toolName: "read", isError: false, content: [{ type: "text", text: "…" }] },
      ]),
    ]);
    expect(updates.map((update) => update.sessionUpdate)).toEqual([
      "user_message_chunk",
      "agent_thought_chunk",
      "agent_message_chunk",
      "tool_call",
      // The settle marker closes the assistant entry before the tool result
      // entry follows.
      "agent_message_chunk",
      "tool_call_update",
    ]);
    const user = updates[0] as Record<string, unknown>;
    expect(user.messageId).toBe("e1");
    expect(user._meta).toEqual({ pi: { at: Date.parse("2026-11-16T08:00:00.000Z"), entryId: "e1" } });
    const thought = updates[1] as Record<string, unknown>;
    expect(thought._meta).toEqual({ pi: { at: Date.parse("2026-11-16T08:00:00.000Z"), entryId: "e2" } });
    const text = updates[2] as Record<string, unknown>;
    expect(text._meta).toEqual({
      pi: {
        at: Date.parse("2026-11-16T08:00:00.000Z"),
        entryId: "e2",
        model: { provider: "dashscope", id: "qwen3.7-max" },
      },
    });
    const toolCall = updates[3] as Record<string, unknown>;
    expect(toolCall.toolCallId).toBe("tc1");
    expect(toolCall.status).toBe("in_progress");
    // ToolCall has no messageId in the schema; grouping is by toolCallId.
    expect(toolCall.messageId).toBeUndefined();
    const settle = updates[4] as Record<string, unknown>;
    expect(settle._meta).toEqual({
      pi: {
        settled: true,
        entryId: "e2",
        at: Date.parse("2026-11-16T08:00:00.000Z"),
        model: { provider: "dashscope", id: "qwen3.7-max" },
        stopReason: "end_turn",
      },
    });
    const result = updates[5] as Record<string, unknown>;
    expect(result.status).toBe("completed");
    expect(result.messageId).toBeUndefined();
    // pi does not persist tool details: a replay ships no `_meta.zu`.
    expect(result._meta).toBeUndefined();
  });

  test("an assistant entry settles even with no displayable text", () => {
    const updates = replayUpdates([
      entry("e1", [{ role: "assistant", content: [{ type: "thinking", thinking: "" }], stopReason: "error", errorMessage: "boom" }]),
    ]);
    expect(updates).toHaveLength(1);
    const settle = updates[0] as Record<string, unknown>;
    expect(settle._meta).toEqual({
      pi: { settled: true, entryId: "e1", at: Date.parse("2026-11-16T08:00:00.000Z"), stopReason: "end_turn", error: "boom" },
    });
  });

  test("a remembered edit replays its diff blocks on the tool result", () => {
    const updates = replayUpdates([
      entry("e1", [
        { role: "assistant", content: [{ type: "toolCall", id: "tc1", name: "edit", arguments: { path: "/a.ts", oldText: "1", newText: "2" } }] },
        { role: "toolResult", toolCallId: "tc1", toolName: "edit", isError: false, content: [] },
      ]),
    ]);
    const result = updates.at(-1) as Record<string, unknown>;
    expect((result.sessionUpdate as string)).toBe("tool_call_update");
    expect(result.content).toEqual([{ type: "diff", path: "/a.ts", oldText: "1", newText: "2" }]);
  });

  test("the message's own timestamp anchors the meta when it has one", () => {
    const updates = replayUpdates([
      entry("e1", [{ role: "user", content: "hi", timestamp: 1760000000123 }]),
    ]);
    expect((updates[0] as Record<string, unknown>)._meta).toEqual({ pi: { at: 1760000000123, entryId: "e1" } });
  });

  test("skips system messages, image blocks and empty thinking", () => {
    const updates = replayUpdates([
      entry("e1", [
        { role: "system", content: "pi prompt line" },
        { role: "user", content: [{ type: "image", data: "aGk=", mimeType: "image/png" }, { type: "text", text: "see this" }] },
      ]),
      entry("e2", [{ role: "assistant", content: [{ type: "thinking", thinking: "" }, { type: "text", text: "ok" }] }]),
    ]);
    expect(updates.map((update) => update.sessionUpdate)).toEqual([
      "user_message_chunk",
      "agent_message_chunk",
      "agent_message_chunk",
    ]);
    const chunk = updates[0] as Record<string, unknown>;
    expect(chunk.content).toEqual({ type: "text", text: "see this" });
  });

  test("marks a tool result with isError failed", () => {
    const updates = replayUpdates([
      entry("e1", [{ role: "toolResult", toolCallId: "tc9", isError: true, content: [{ type: "text", text: "no" }] }]),
    ]);
    const update = updates[0] as Record<string, unknown>;
    expect(update.status).toBe("failed");
  });

  test("replays string user content and unknown roles as registered skips", () => {
    const updates = replayUpdates([
      entry("e1", [{ role: "user", content: "plain" }]),
      entry("e2", [{ role: "custom_message", content: "nope" }]),
    ]);
    expect(updates).toHaveLength(1);
    expect((updates[0] as Record<string, unknown>).messageId).toBe("e1");
  });
});
