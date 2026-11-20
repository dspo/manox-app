import { describe, expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ModelRuntime, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { assertFreeSessionId, modelMeta, PermissionGate, sessionConfigOptions, type ConfigOptionsRuntime } from "./pi-session";

/**
 * A1 review: the approval gate treats `ask_user_question` as read-only —
 * it opens no approval card in ask mode and survives read-only's write
 * block. Both paths go through the `tool_call` handler the gate registers,
 * the way pi drives it.
 */

const noop = () => {};

/** Registers the gate's extension on a fake pi and returns its `tool_call` handler. */
function handlerFor(mode: string): (event: unknown) => Promise<unknown> {
  const gate = new PermissionGate(noop);
  gate.mode = mode;
  let handler: ((...args: unknown[]) => unknown) | undefined;
  gate.extension()({
    on(event: string, fn: (...args: unknown[]) => unknown) {
      if (event === "tool_call") handler = fn;
      return () => {};
    },
  } as unknown as ExtensionAPI);
  return handler as (event: unknown) => Promise<unknown>;
}

describe("the approval gate and ask_user_question", () => {
  test("ask mode: the question tool runs without an approval card", async () => {
    const handler = handlerFor("ask");
    // No bridge attached: a workspace tool would demand approval and be
    // blocked; the question tool must simply pass.
    expect(await handler({ toolName: "ask_user_question" })).toBeUndefined();
    expect(await handler({ toolName: "edit" })).toEqual({
      block: true,
      reason: "no approval channel is attached",
    });
  });

  test("read-only mode: the question tool survives the write block", async () => {
    const handler = handlerFor("read-only");
    expect(await handler({ toolName: "ask_user_question" })).toBeUndefined();
    expect(await handler({ toolName: "bash" })).toEqual({
      block: true,
      reason: expect.stringContaining("read-only mode"),
    });
  });

  test("auto mode: everything passes, questions included", async () => {
    const handler = handlerFor("auto");
    expect(await handler({ toolName: "ask_user_question" })).toBeUndefined();
    expect(await handler({ toolName: "bash" })).toBeUndefined();
  });

  test("read-only mode: the plan tools keep the plan honest (A2)", async () => {
    // The todo_* tools write the session's own state file, not the workspace;
    // blocking them would freeze the visible plan mid-run.
    const handler = handlerFor("read-only");
    for (const tool of ["todo_list", "todo_add", "todo_update", "todo_promote", "todo_remove", "todo_write"]) {
      expect(await handler({ toolName: tool })).toBeUndefined();
    }
    expect(await handler({ toolName: "write" })).toEqual({
      block: true,
      reason: expect.stringContaining("read-only mode"),
    });
  });
});

// The config selectors the harness advertises (issue #54): each model
// option carries its provider's facts in `_meta`, so the menu answers
// "why is this provider listed" without a second catalog process.

describe("sessionConfigOptions", () => {
  const runtime: ConfigOptionsRuntime = {
    getAvailableSnapshot: () => [
      { provider: "anthropic", id: "claude-x", name: "Claude X", contextWindow: 200_000 },
      { provider: "bailian", id: "qwen-f", name: "Qwen F", contextWindow: 1_000_000 },
    ],
    getProvider: (id) => (id === "anthropic" ? { name: "Anthropic" } : undefined),
    getProviderAuthStatus: (id) =>
      id === "anthropic"
        ? { configured: true, source: "stored" }
        : id === "bailian"
          ? { configured: true, source: "environment", label: "DASHSCOPE_API_KEY" }
          : { configured: false },
  };

  test("each model option carries its provider's display name, source and context", () => {
    const options = sessionConfigOptions(runtime, null, { provider: "anthropic", id: "claude-x" }, ["off", "high"], "off");
    const model = options.find((option) => option.id === "model");
    const byValue = new Map(model?.options.map((option) => [option.value, option]));
    expect(byValue.get("anthropic/claude-x")?._meta).toEqual({
      providerName: "Anthropic",
      authSource: "stored",
      contextWindow: 200_000,
    });
    // A provider without a registered display name keeps its id; the
    // environment source names the variable it reads.
    expect(byValue.get("bailian/qwen-f")?._meta).toEqual({
      providerName: "bailian",
      authSource: "environment",
      authLabel: "DASHSCOPE_API_KEY",
      contextWindow: 1_000_000,
    });
    expect(model?.currentValue).toBe("anthropic/claude-x");
  });

  test("pi 1.5.0's real ModelRuntime supplies the auth status the menu reads", async () => {
    const authPath = `/tmp/zu-auth-status-${Date.now()}-${Math.floor(Math.random() * 1e6)}.json`;
    const real = await ModelRuntime.create({ authPath, modelsPath: null, allowModelNetwork: false });
    const runtime = real as ConfigOptionsRuntime;
    const unconfigured = runtime.getProviderAuthStatus("definitely-not-a-provider");
    expect(unconfigured.configured).toBe(false);

    // A runtime API key is the same source the CLI's auth check sees. The
    // menu must not report `none` for a provider that is already usable.
    await real.setRuntimeApiKey("openai", "sk-test");
    const configured = runtime.getProviderAuthStatus("openai");
    expect(configured.configured).toBe(true);
    expect(configured.source).toBe("runtime");
    expect(modelMeta(runtime, "openai")?.authSource).toBe("runtime");
  });

  test("an unconfigured provider is listed as none, not fake-signed-in", () => {
    const bare: ConfigOptionsRuntime = {
      getAvailableSnapshot: () => [{ provider: "p", id: "m", name: "M", contextWindow: 8 }],
      getProviderAuthStatus: () => ({ configured: false }),
    };
    const options = sessionConfigOptions(bare, null, undefined, [], "off");
    expect(options.find((option) => option.id === "model")?.options[0]._meta).toEqual({
      providerName: "p",
      authSource: "none",
      contextWindow: 8,
    });
    // No thought ladder, no thought_level selector.
    expect(options.some((option) => option.id === "thought_level")).toBe(false);
  });

  test("the session's restored model stays selectable with its own _meta", () => {
    const scoped: ConfigOptionsRuntime = {
      getAvailableSnapshot: () => [{ provider: "other", id: "m2", name: "M2" }],
      getProvider: () => undefined,
      getProviderAuthStatus: () => ({ configured: true, source: "models_json_key" }),
    };
    const options = sessionConfigOptions(
      scoped,
      ["other/*"],
      { provider: "anthropic", id: "claude-x", name: "Claude X" },
      ["off"],
      "off",
    );
    const model = options.find((option) => option.id === "model");
    expect(model?.options.map((option) => option.value)).toEqual(["anthropic/claude-x", "other/m2"]);
    expect(model?.options[0]._meta).toEqual({ providerName: "anthropic", authSource: "models_json_key" });
  });
});

// The client-minted session id (issue #56): `session/new` `_meta.sessionId`
// must be a UUID — client v4 and pi's own v7 alike — and must not collide
// with an existing session file, so an empty rebuild can never shadow a
// real session's history.

describe("assertFreeSessionId", () => {
  const agentDir = `/tmp/zu-session-id-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

  test("a free UUID passes", () => {
    expect(() => assertFreeSessionId("eaaa1111-0000-4000-8000-00000000000a", agentDir)).not.toThrow();
  });

  test("a non-UUID is refused before pi sees it", () => {
    expect(() => assertFreeSessionId("stub-0000-0001", agentDir)).toThrow(/is not a UUID/);
    expect(() => assertFreeSessionId("../escape", agentDir)).toThrow(/is not a UUID/);
  });

  test("an id that already has a session file is refused", () => {
    const id = "ebbb2222-0000-4000-8000-00000000000b";
    mkdirSync(join(agentDir, "sessions", "--tmp-x--"), { recursive: true });
    writeFileSync(
      join(agentDir, "sessions", "--tmp-x--", `2026-11-19T00-00-00-000Z_${id}.jsonl`),
      `{"type":"session","version":3,"id":"${id}"}\n`,
    );
    expect(() => assertFreeSessionId(id, agentDir)).toThrow(/already has a session file/);
  });
});
