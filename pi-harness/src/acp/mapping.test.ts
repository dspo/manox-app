/**
 * Unit tests for the pure pi → ACP mappings: prompt content blocks, tool
 * card shapes, streaming deltas and stop reasons. No pi, no network.
 */

import { describe, expect, test } from "bun:test";
import {
  assistantDeltaUpdate,
  describeToolCall,
  editDiffBlocks,
  piToolKind,
  promptBlocksToText,
  settledUpdate,
  stopReasonFromPi,
  toolCallEndUpdate,
  toolCallPartialUpdate,
  toolCallStartUpdate,
  toolStats,
  userEntryUpdates,
} from "./mapping";

describe("promptBlocksToText", () => {
  test("joins text blocks", () => {
    const result = promptBlocksToText([
      { type: "text", text: "hello " },
      { type: "text", text: "world" },
    ]);
    expect(result).toEqual({ ok: true, text: "hello world" });
  });

  test("renders resource links as markdown (baseline content)", () => {
    const result = promptBlocksToText([{ type: "resource_link", uri: "file:///tmp/a.ts", name: "a.ts" }]);
    expect(result).toEqual({ ok: true, text: "[a.ts](file:///tmp/a.ts)" });
  });

  test("resource link without a name uses the uri", () => {
    const result = promptBlocksToText([{ type: "resource_link", uri: "https://example.com" }]);
    expect(result).toEqual({ ok: true, text: "[https://example.com](https://example.com)" });
  });

  test("rejects image blocks with a coded invalid-params error", () => {
    const result = promptBlocksToText([{ type: "image", data: "aGk=", mimeType: "image/png" }]);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error.code).toBe(-32602);
      expect((result.error.data as { code: string }).code).toBe("image_not_supported");
    }
  });

  test("rejects audio and embedded resources with their own codes", () => {
    const audio = promptBlocksToText([{ type: "audio", data: "aGk=", mimeType: "audio/wav" }]);
    expect(audio.ok).toBe(false);
    expect(audio.ok ? "" : (audio.error.data as { code: string }).code).toBe("audio_not_supported");
    const resource = promptBlocksToText([
      { type: "resource", resource: { uri: "file:///a", text: "x" } },
    ] as never);
    expect(resource.ok).toBe(false);
    expect(resource.ok ? "" : (resource.error.data as { code: string }).code).toBe("embedded_context_not_supported");
  });

  test("rejects an empty prompt", () => {
    expect(promptBlocksToText([]).ok).toBe(false);
    expect(promptBlocksToText([{ type: "text", text: "   " }]).ok).toBe(false);
  });
});

describe("piToolKind", () => {
  test("maps the known pi tools onto ACP kinds", () => {
    expect(piToolKind("read")).toBe("read");
    expect(piToolKind("grep")).toBe("search");
    expect(piToolKind("edit")).toBe("edit");
    expect(piToolKind("bash")).toBe("execute");
    expect(piToolKind("think")).toBe("think");
    expect(piToolKind("webfetch")).toBe("fetch");
    expect(piToolKind("mystery_tool")).toBe("other");
  });
});

describe("describeToolCall", () => {
  test("quotes the most salient string argument", () => {
    expect(describeToolCall("read", { path: "/tmp/a.ts", line: 3 })).toBe("read /tmp/a.ts");
    expect(describeToolCall("bash", { command: "bun test\nmore" })).toBe("bash bun test");
  });

  test("falls back to the bare tool name", () => {
    expect(describeToolCall("bash", undefined)).toBe("bash");
    expect(describeToolCall("bash", { nested: { a: 1 } })).toBe("bash");
  });
});

describe("tool call updates", () => {
  test("start opens an in_progress card with title, kind and rawInput", () => {
    const update = toolCallStartUpdate("tc1", "edit", { path: "/tmp/a.ts", content: "x" }) as Record<string, unknown>;
    expect(update.sessionUpdate).toBe("tool_call");
    expect(update.toolCallId).toBe("tc1");
    expect(update.status).toBe("in_progress");
    expect(update.title).toBe("edit /tmp/a.ts");
    expect(update.kind).toBe("edit");
    expect(update.locations).toEqual([{ path: "/tmp/a.ts" }]);
  });

  test("partial update carries text when the partial has any", () => {
    const update = toolCallPartialUpdate("tc1", { content: [{ type: "text", text: "partial" }] });
    expect(update).toEqual({
      sessionUpdate: "tool_call_update",
      toolCallId: "tc1",
      status: "in_progress",
      content: [{ type: "content", content: { type: "text", text: "partial" } }],
    });
    expect(toolCallPartialUpdate("tc1", { quiet: true })).toBeUndefined();
  });

  test("end settles the card failed or completed with the result text", () => {
    const failed = toolCallEndUpdate("tc1", "edit", { content: [{ type: "text", text: "boom" }] }, true) as Record<
      string,
      unknown
    >;
    expect(failed.status).toBe("failed");
    expect(failed.content).toEqual([{ type: "content", content: { type: "text", text: "boom" } }]);
    const done = toolCallEndUpdate("tc2", "read", {}, false) as Record<string, unknown>;
    expect(done.status).toBe("completed");
    expect(done.content).toBeUndefined();
    expect(done._meta).toBeUndefined();
  });

  test("end turns edit arguments into ToolCallContent.diff blocks", () => {
    const update = toolCallEndUpdate(
      "tc1",
      "edit",
      {},
      false,
      { path: "/tmp/a.ts", oldText: "a", newText: "b" },
    ) as Record<string, unknown>;
    expect(update.content).toEqual([
      { type: "diff", path: "/tmp/a.ts", oldText: "a", newText: "b" },
    ]);
    // read/bash carry no diff; a multi-op edit makes one block per op.
    expect(
      editDiffBlocks("read", { path: "/a.ts", newText: "x" }),
    ).toEqual([]);
    expect(editDiffBlocks("edit", { path: "/a.ts", edits: [{ oldText: "1", newText: "2" }, { newText: "3" }] })).toEqual([
      { type: "diff", path: "/a.ts", oldText: "1", newText: "2" },
      { type: "diff", path: "/a.ts", oldText: null, newText: "3" },
    ]);
  });

  test("end frames carry _meta.zu: duration, exit code and truncation", () => {
    const stats = toolStats(1820, {
      structuredContent: { exit_code: 1 },
      details: { truncation: { truncated: true, totalLines: 36 }, fullOutputPath: "/tmp/pi-bash-out" },
    });
    const update = toolCallEndUpdate("tc1", "bash", {}, true, { command: "bun test" }, stats) as Record<
      string,
      unknown
    >;
    expect(update._meta).toEqual({
      zu: { durationMs: 1820, exitCode: 1, truncated: true, totalLines: 36, fullOutputPath: "/tmp/pi-bash-out" },
    });
    // Absent facts are dropped, and a bare result ships no meta at all.
    expect(toolStats(undefined, {}).durationMs).toBeUndefined();
    expect(toolStats(12, { structuredContent: { exit_code: "nope" } }).exitCode).toBeUndefined();
    expect(toolStats(12, {}).truncated).toBeUndefined();
    const plain = toolCallEndUpdate("tc2", "read", {}, false, undefined, toolStats(undefined, {})) as Record<
      string,
      unknown
    >;
    expect(plain._meta).toBeUndefined();
  });

  test("partial frames never carry the stats", () => {
    const partial = toolCallPartialUpdate("tc1", { text: "streaming" });
    expect(partial && "_meta" in partial).toBe(false);
  });

  test("end carries plain object details under _meta.zu", () => {
    const details = { options: [{ id: "a", label: "A" }] };
    const update = toolCallEndUpdate("tc1", "tool", { content: [{ type: "text", text: "ok" }], details }, false) as Record<
      string,
      unknown
    >;
    expect((update._meta as { zu: { details: unknown } }).zu.details).toEqual(details);
  });

  test("end without details carries no _meta", () => {
    for (const result of [{}, { content: [] }, { details: undefined }, { details: null }]) {
      const update = toolCallEndUpdate("tc1", "tool", result, false) as Record<string, unknown>;
      expect(update._meta).toBeUndefined();
    }
  });

  test("scalar details drop — no guessing at a shape", () => {
    for (const details of ["flat text", 7, true]) {
      const update = toolCallEndUpdate("tc1", "tool", { details }, false) as Record<string, unknown>;
      expect(update._meta).toBeUndefined();
    }
  });

  test("details past the 64 KiB cap drop whole instead of truncating", () => {
    const update = toolCallEndUpdate("tc1", "tool", { details: { blob: "x".repeat(80 * 1024) } }, false) as Record<
      string,
      unknown
    >;
    expect(update._meta).toBeUndefined();
  });

  test("circular details drop instead of throwing", () => {
    const circular: Record<string, unknown> = {};
    circular.self = circular;
    const update = toolCallEndUpdate("tc1", "tool", { details: circular }, false) as Record<string, unknown>;
    expect(update._meta).toBeUndefined();
  });
});

describe("assistantDeltaUpdate", () => {
  test("maps text and thinking deltas onto the chunk updates", () => {
    expect(assistantDeltaUpdate({ type: "text_delta", delta: "hi" }, "m1")).toEqual({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hi" },
      messageId: "m1",
    });
    expect(assistantDeltaUpdate({ type: "thinking_delta", delta: "hmm" }, "m2")).toEqual({
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: "hmm" },
      messageId: "m2",
    });
  });

  test("ignores non-chunk deltas and empty ones", () => {
    expect(assistantDeltaUpdate({ type: "toolcall_delta", delta: "{}" }, "m1")).toBeUndefined();
    expect(assistantDeltaUpdate({ type: "text_delta", delta: "" }, "m1")).toBeUndefined();
    expect(assistantDeltaUpdate(undefined, "m1")).toBeUndefined();
  });

  test("rides the anchor facts as _meta.pi on every chunk", () => {
    const meta = { at: 1760000000000, model: { provider: "dashscope", id: "qwen3.7-max" } };
    expect(assistantDeltaUpdate({ type: "text_delta", delta: "hi" }, "m1", meta)).toEqual({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hi" },
      messageId: "m1",
      _meta: { pi: meta },
    });
    expect(assistantDeltaUpdate({ type: "thinking_delta", delta: "hmm" }, "m1", meta)?._meta).toEqual({ pi: meta });
    // An all-absent meta ships nothing.
    expect(assistantDeltaUpdate({ type: "text_delta", delta: "hi" }, "m1", {})?._meta).toBeUndefined();
  });
});

describe("settledUpdate", () => {
  test("is an empty chunk whose _meta.pi carries the settle facts", () => {
    const update = settledUpdate({
      entryId: "e1",
      at: 2000,
      durationMs: 900,
      stopReason: "end_turn",
      error: "boom",
    }) as Record<string, unknown>;
    expect(update.sessionUpdate).toBe("agent_message_chunk");
    expect(update.content).toEqual({ type: "text", text: "" });
    expect(update.messageId).toBe("e1");
    expect(update._meta).toEqual({
      pi: { settled: true, entryId: "e1", at: 2000, durationMs: 900, stopReason: "end_turn", error: "boom" },
    });
    // The bare marker still names its entry.
    const minimal = settledUpdate({ entryId: "e2" }) as Record<string, unknown>;
    expect(minimal._meta).toEqual({ pi: { settled: true, entryId: "e2" } });
  });
});

describe("userEntryUpdates", () => {
  test("sends the joined text under the entry id with _meta.pi", () => {
    expect(userEntryUpdates("e7", "count slowly", 42)).toEqual([
      {
        sessionUpdate: "user_message_chunk",
        content: { type: "text", text: "count slowly" },
        messageId: "e7",
        _meta: { pi: { at: 42, entryId: "e7" } },
      }]);
    expect(userEntryUpdates("e8", [{ type: "text", text: "a" }, { type: "text", text: "b" }], 7)).toEqual([
      {
        sessionUpdate: "user_message_chunk",
        content: { type: "text", text: "ab" },
        messageId: "e8",
        _meta: { pi: { at: 7, entryId: "e8" } },
      }]);
    // No text, no update (the harness accepts text prompts only).
    expect(userEntryUpdates("e9", "", 7)).toEqual([]);
  });
});

describe("stopReasonFromPi", () => {
  test("maps the settled pi stop reasons", () => {
    expect(stopReasonFromPi("stop")).toBe("end_turn");
    expect(stopReasonFromPi("length")).toBe("max_tokens");
    expect(stopReasonFromPi("aborted")).toBe("cancelled");
  });

  test("maps the shapeless ones to end_turn", () => {
    for (const reason of ["error", "toolUse", "pending", "deferred", undefined, "something_new"]) {
      expect(stopReasonFromPi(reason as string | undefined)).toBe("end_turn");
    }
  });
});
