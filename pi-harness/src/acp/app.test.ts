/**
 * Unit tests for the ACP method surface, driven over real ndjson byte
 * streams: framing (split lines, line cap behavior), JSON-RPC id pairing,
 * protocol-level error responses, and the method semantics with fake
 * sessions. No pi, no network.
 */

import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ndJsonStream, type SessionUpdate, type StopReason } from "@agentclientprotocol/sdk";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { applyTodoOp, EMPTY_PLAN, readPlan } from "../todos/core";
import { DISPATCH_RULE } from "../extensions/prompt-section";
import { ARTIFACTS_RULE } from "../extensions/artifacts";
import { hostExtensionFactories } from "./pi-session";
import {
  createAcpApp,
  type HarnessClientBridge,
  type HarnessPorts,
  type HarnessSession,
  type SessionBanner,
} from "./app";
import { SubagentManager } from "./subagents";

// ---------------------------------------------------------------- harness

class FakeSession implements HarnessSession {
  readonly id: string;
  prompts: string[] = [];
  aborted = 0;
  disposed = 0;
  readonly configChanges: Array<{ configId: string; value: string }> = [];
  readonly modeChanges: string[] = [];
  readonly permissionAnswers: unknown[] = [];
  readonly elicitationAnswers: unknown[] = [];
  private listeners: Array<(update: SessionUpdate) => void> = [];
  private releasePrompt: (() => void) | undefined;
  private bridge: HarnessClientBridge | null = null;

  constructor(
    private readonly spec: {
      id: string;
      updates?: SessionUpdate[];
      replay?: SessionUpdate[];
      stop?: StopReason;
      /** The prompt promise holds until `abort()` or `settlePrompt()`. */
      hangUntilAbort?: boolean;
      /** Asks the client for approval before answering. */
      permission?: boolean;
      /** Asks the client for structured input before answering. */
      elicit?: boolean;
      subagents?: HarnessSession["subagents"];
      answers?: HarnessSession["answers"];
      todos?: HarnessSession["todos"];
      /** The banner facts the announce step sends (absent = no banner). */
      banner?: SessionBanner | null;
    },
  ) {
    this.id = spec.id;
  }

  banner(): SessionBanner | undefined {
    return this.spec.banner ?? undefined;
  }

  async prompt(text: string): Promise<StopReason> {
    this.prompts.push(text);
    if (this.spec.permission && this.bridge) {
      this.permissionAnswers.push(
        await this.bridge.requestPermission({
          sessionId: this.id,
          toolCall: { toolCallId: "tc-ask", title: "run the dangerous thing", kind: "execute" },
          options: [
            { optionId: "allow_once", name: "Allow once", kind: "allow_once" },
            { optionId: "reject", name: "Reject", kind: "reject_once" },
          ],
        }),
      );
    }
    if (this.spec.elicit && this.bridge) {
      this.elicitationAnswers.push(
        await this.bridge.elicit({
          sessionId: this.id,
          message: "Which environment?",
          requestedSchema: {
            type: "object",
            properties: { environment: { type: "string", enum: ["staging", "prod"] } },
            required: ["environment"],
          },
        }),
      );
    }
    for (const update of this.spec.updates ?? []) {
      for (const listener of this.listeners) listener(update);
    }
    if (this.spec.hangUntilAbort) {
      await new Promise<void>((resolve) => {
        this.releasePrompt = resolve;
      });
      return this.spec.stop ?? "cancelled";
    }
    return this.spec.stop ?? "end_turn";
  }

  async abort(): Promise<void> {
    this.aborted += 1;
    this.releasePrompt?.();
  }

  dispose(): void {
    this.disposed += 1;
  }

  replay(): SessionUpdate[] {
    return this.spec.replay ?? [];
  }

  onUpdate(push: (update: SessionUpdate) => void): void {
    this.listeners.push(push);
  }

  configOptions() {
    return [
      {
        id: "model",
        name: "Model",
        category: "model" as const,
        type: "select" as const,
        currentValue: this.configChanges.at(-1)?.value ?? "p/m",
        options: [
          { value: "p/m", name: "p/m" },
          { value: "p/other", name: "p/other" },
        ],
      },
    ];
  }

  availableCommands() {
    return [{ name: "compact", description: "Compact the context." }];
  }

  async setConfigOption(configId: string, value: string) {
    this.configChanges.push({ configId, value });
    return this.configOptions();
  }

  modeState() {
    return {
      currentModeId: this.modeChanges.at(-1) ?? "ask",
      availableModes: [
        { id: "ask", name: "Ask" },
        { id: "auto", name: "Auto" },
      ],
    };
  }

  async setMode(modeId: string): Promise<void> {
    this.modeChanges.push(modeId);
    for (const listener of this.listeners) {
      listener({ sessionUpdate: "current_mode_update", currentModeId: modeId } as SessionUpdate);
    }
  }

  attachClient(bridge: HarnessClientBridge | null): void {
    this.bridge = bridge;
  }

  get subagents(): HarnessSession["subagents"] {
    return this.spec.subagents;
  }

  get answers(): HarnessSession["answers"] {
    return this.spec.answers;
  }

  get todos(): HarnessSession["todos"] {
    return this.spec.todos;
  }
}

interface PortsSpy {
  created: string[];
  loaded: Array<{ sessionId: string; cwd: string }>;
  resumed: Array<{ sessionId: string; cwd: string }>;
  createdContexts: Array<{ elicit?: unknown; mcpServers?: unknown[] }>;
}

function fakePorts(sessions: FakeSession[], onSession?: (session: FakeSession, spy: PortsSpy) => void): {
  ports: HarnessPorts;
  spy: PortsSpy;
} {
  const spy: PortsSpy = { created: [], loaded: [], resumed: [], createdContexts: [] };
  let index = 0;
  return {
    spy,
    ports: {
      createSession: async (cwd, context) => {
        spy.created.push(cwd);
        spy.createdContexts.push(context ?? {});
        const session = sessions[index++] ?? new FakeSession({ id: `extra-${index}` });
        onSession?.(session, spy);
        return session;
      },
      loadSession: async (sessionId, cwd) => {
        spy.loaded.push({ sessionId, cwd });
        const session = sessions[index++] ?? new FakeSession({ id: sessionId });
        onSession?.(session, spy);
        return session;
      },
      resumeSession: async (sessionId, cwd) => {
        spy.resumed.push({ sessionId, cwd });
        const session = sessions[index++] ?? new FakeSession({ id: sessionId });
        onSession?.(session, spy);
        return session;
      },
    },
  };
}

/** A raw byte-level ACP client over the same ndjson framing the agent uses. */
function startClient(ports: HarnessPorts): ByteClient {
  const logs: string[] = [];
  return startClientWithApp(createAcpApp(ports, (line) => logs.push(line)), logs);
}

function startClientWithApp(app: ReturnType<typeof createAcpApp>, logs: string[] = []): ByteClient {
  const toAgent = new TransformStream<Uint8Array, Uint8Array>();
  const fromAgent = new TransformStream<Uint8Array, Uint8Array>();
  const connection = app.connect(ndJsonStream(fromAgent.writable, toAgent.readable));
  const writer = toAgent.writable.getWriter();
  const reader = fromAgent.readable.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const pending: Array<Record<string, unknown>> = [];
  const waiters: Array<(message: Record<string, unknown>) => void> = [];
  const sideChannel: Array<Record<string, unknown>> = [];

  void (async () => {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        const line = buffer.slice(0, newline);
        buffer = buffer.slice(newline + 1);
        if (line.trim().length > 0) {
          const message = JSON.parse(line) as Record<string, unknown>;
          const waiter = waiters.shift();
          if (waiter) waiter(message);
          else pending.push(message);
        }
        newline = buffer.indexOf("\n");
      }
    }
  })();

  let nextId = 0;

  const client: ByteClient = {
    logs,
    connection,
    async write(text: string): Promise<void> {
      await writer.write(new TextEncoder().encode(text));
    },
    async send(message: unknown): Promise<void> {
      await this.write(`${JSON.stringify(message)}\n`);
    },
    /** Next protocol message, in arrival order (responses and notifications). */
    next(): Promise<Record<string, unknown>> {
      const message = pending.shift();
      if (message) return Promise.resolve(message);
      return new Promise((resolve) => waiters.push(resolve));
    },
    /** Sends a request and resolves with its paired response. */
    async request(method: string, params?: unknown): Promise<Record<string, unknown>> {
      const id = ++nextId;
      await this.send({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
      return this.waitResponse(id);
    },
    /** Sends a request whose response is awaited separately. */
    async requestNoWait(method: string, params?: unknown): Promise<number> {
      const id = ++nextId;
      await this.send({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
      return id;
    },
    /** Reads until the response for `id`, keeping notifications aside. */
    async waitResponse(id: number | string): Promise<Record<string, unknown>> {
      while (true) {
        const message = await this.next();
        if ("id" in message && message.id === id && ("result" in message || "error" in message)) return message;
        sideChannel.push(message);
      }
    },
    /** The notifications set aside while waiting for responses. */
    drained(): Array<Record<string, unknown>> {
      return sideChannel.splice(0);
    },
    async close(): Promise<void> {
      await writer.close();
    },
  };
  return client;
}

interface ByteClient {
  logs: string[];
  connection: { close(error?: unknown): void };
  write(text: string): Promise<void>;
  send(message: unknown): Promise<void>;
  next(): Promise<Record<string, unknown>>;
  request(method: string, params?: unknown): Promise<Record<string, unknown>>;
  requestNoWait(method: string, params?: unknown): Promise<number>;
  waitResponse(id: number | string): Promise<Record<string, unknown>>;
  drained(): Array<Record<string, unknown>>;
  close(): Promise<void>;
}

const INITIALIZE_PARAMS = { protocolVersion: 1, clientCapabilities: {} };

async function initialize(client: ReturnType<typeof startClient>): Promise<Record<string, unknown>> {
  const response = await client.request("initialize", INITIALIZE_PARAMS);
  expect(response.error).toBeUndefined();
  return response.result as Record<string, unknown>;
}

/** Reads the stream until the agent asks the client something. */
async function nextAgentRequest(
  client: ByteClient,
  method: string,
): Promise<{ id: number; params: Record<string, unknown> }> {
  while (true) {
    const message = await client.next();
    if (message.method === method && typeof message.id === "number") {
      return { id: message.id, params: message.params as Record<string, unknown> };
    }
  }
}

// ----------------------------------------------------------------- framing

describe("framing and id pairing", () => {
  test("answers initialize over split byte chunks", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    const payload = `${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: INITIALIZE_PARAMS })}\n`;
    // One frame, three arbitrary cuts.
    await client.write(payload.slice(0, 9));
    await client.write(payload.slice(9, 40));
    await client.write(payload.slice(40));
    const response = await client.waitResponse(1);
    expect((response.result as Record<string, unknown>).protocolVersion).toBe(1);
    await client.close();
  });

  test("pairs two pipelined requests written as one chunk", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    await client.request("initialize", INITIALIZE_PARAMS);
    const batch = [
      JSON.stringify({ jsonrpc: "2.0", id: 100, method: "no/such/method" }),
      JSON.stringify({ jsonrpc: "2.0", id: 200, method: "another/missing", params: {} }),
    ].join("\n");
    await client.write(`${batch}\n`);
    const first = await client.waitResponse(100);
    const second = await client.waitResponse(200);
    expect((first.error as { code: number }).code).toBeDefined();
    expect((second.error as { code: number }).code).toBeDefined();
    expect(first.id).toBe(100);
    expect(second.id).toBe(200);
    await client.close();
  });

  test("a malformed json line draws a parse error with a null id", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    await client.request("initialize", INITIALIZE_PARAMS);
    await client.write("{this is not json}\n");
    const error = (await client.next()) as { id: unknown; error?: { code: number } };
    expect(error.id).toBeNull();
    expect(error.error?.code).toBe(-32700);
    await client.close();
  });

  test("a json array (ACP has no batches) draws -32600 with a null id", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    await client.request("initialize", INITIALIZE_PARAMS);
    await client.write('[{"jsonrpc":"2.0","id":7,"method":"initialize"}]\n');
    const error = (await client.next()) as { id: unknown; error?: { code: number } };
    expect(error.id).toBeNull();
    expect(error.error?.code).toBe(-32600);
    await client.close();
  });

  test("an unknown method is a coded method-not-found error", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    await client.request("initialize", INITIALIZE_PARAMS);
    const response = await client.request("session/definitely_not_supported");
    const error = response.error as { code: number; data?: { code?: string } };
    expect(error.code).toBe(-32601);
    // Repo discipline: even a protocol-level miss names a string code.
    expect(typeof error.data?.code).toBe("string");
    await client.close();
  });
});

// ----------------------------------------------------------- method surface

describe("method surface", () => {
  test("initialize declares loadSession, no image prompt support and no auth methods", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    const result = await initialize(client);
    expect(result.protocolVersion).toBe(1);
    expect(result.authMethods).toEqual([]);
    const capabilities = result.agentCapabilities as Record<string, unknown>;
    expect(capabilities.loadSession).toBe(true);
    expect((capabilities.promptCapabilities as Record<string, unknown>).image).toBe(false);
    expect((result.agentInfo as Record<string, unknown>).name).toBe("pi-harness");
    await client.close();
  });

  test("session/new answers pi's sessionId and carries mcpServers into the ports", async () => {
    const { ports, spy } = fakePorts([new FakeSession({ id: "pi-1" })]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/new", {
      cwd: "/tmp/proj",
      mcpServers: [{ type: "stdio", name: "noise", command: "/bin/echo", args: [], env: [] }],
    });
    expect(response.error).toBeUndefined();
    expect((response.result as Record<string, unknown>).sessionId).toBe("pi-1");
    expect(spy.created).toEqual(["/tmp/proj"]);
    expect(spy.createdContexts[0]?.mcpServers).toEqual([
      { type: "stdio", name: "noise", command: "/bin/echo", args: [], env: [] },
    ]);
    expect(typeof spy.createdContexts[0]?.elicit).toBe("function");
    await client.close();
  });

  test("session/new carries a client-minted id from `_meta` into the ports (issue #56)", async () => {
    const minted = "dddd3333-0000-4000-8000-000000000003";
    const { ports, spy } = fakePorts([new FakeSession({ id: minted })]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/new", {
      cwd: "/tmp/proj",
      _meta: { sessionId: minted },
    });
    expect(response.error).toBeUndefined();
    // The response names the client's id: the view key and the session file
    // are the same handle from the first frame.
    expect((response.result as Record<string, unknown>).sessionId).toBe(minted);
    expect((spy.createdContexts[0] as { sessionId?: string }).sessionId).toBe(minted);
    await client.close();
  });

  test("session/new without `_meta` leaves the mint to the ports (standard ACP)", async () => {
    const { ports, spy } = fakePorts([new FakeSession({ id: "pi-minted-here" })]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/new", { cwd: "/tmp/proj" });
    expect(response.error).toBeUndefined();
    expect(spy.createdContexts[0]?.sessionId).toBeUndefined();
    await client.close();
  });

  test("session/prompt streams updates in order, then settles", async () => {
    const session = new FakeSession({
      id: "pi-2",
      updates: [
        { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hel" }, messageId: "m1" },
        { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "lo" }, messageId: "m1" },
      ],
    });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const response = await client.request("session/prompt", {
      sessionId: "pi-2",
      prompt: [{ type: "text", text: "say hi" }],
    });
    expect((response.result as Record<string, unknown>).stopReason).toBe("end_turn");
    const updates = client
      .drained()
      .filter((message) => message.method === "session/update")
      .filter(
        (message) =>
          ((message.params as Record<string, unknown>).update as Record<string, unknown>).sessionUpdate !==
          "available_commands_update",
      );
    expect(updates).toHaveLength(2);
    expect((updates[0].params as Record<string, unknown>).sessionId).toBe("pi-2");
    expect(((updates[0].params as Record<string, unknown>).update as Record<string, unknown>).sessionUpdate).toBe(
      "agent_message_chunk",
    );
    expect(session.prompts).toEqual(["say hi"]);
    await client.close();
  });

  test("an image prompt is a coded invalid-params error", async () => {
    const { ports } = fakePorts([new FakeSession({ id: "pi-3" })]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const response = await client.request("session/prompt", {
      sessionId: "pi-3",
      prompt: [{ type: "image", data: "aGk=", mimeType: "image/png" }],
    });
    const error = response.error as { code: number; data: { code: string } };
    expect(error.code).toBe(-32602);
    expect(error.data.code).toBe("image_not_supported");
    await client.close();
  });

  test("an unknown session is -32002 with the sessionId in data", async () => {
    const { ports } = fakePorts([]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/prompt", {
      sessionId: "ghost",
      prompt: [{ type: "text", text: "hi" }],
    });
    const error = response.error as { code: number; data: { code: string; sessionId: string } };
    expect(error.code).toBe(-32002);
    expect(error.data.code).toBe("session_not_found");
    expect(error.data.sessionId).toBe("ghost");
    await client.close();
  });

  test("cancel aborts the hanging turn, which answers cancelled; a second prompt is busy", async () => {
    const session = new FakeSession({ id: "pi-4", hangUntilAbort: true, stop: "cancelled" });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    // The first prompt hangs until the abort; don't await it yet.
    const id = await client.requestNoWait("session/prompt", {
      sessionId: "pi-4",
      prompt: [{ type: "text", text: "long" }],
    });
    // While it runs, another prompt on the same session is refused…
    const busy = await client.request("session/prompt", { sessionId: "pi-4", prompt: [{ type: "text", text: "again" }] });
    expect((busy.error as { data: { code: string } }).data.code).toBe("session_busy");
    // …and the cancel notification (no reply of its own) settles the turn.
    await client.send({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId: "pi-4" } });
    const response = await client.waitResponse(id);
    expect((response.result as Record<string, unknown>).stopReason).toBe("cancelled");
    expect(session.aborted).toBe(1);
    await client.close();
  });

  test("session/load replays history updates before the response", async () => {
    const session = new FakeSession({
      id: "pi-5",
      replay: [
        { sessionUpdate: "user_message_chunk", content: { type: "text", text: "earlier question" }, messageId: "e1" },
        { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "earlier answer" }, messageId: "e2" },
      ],
    });
    const { ports, spy } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/load", { sessionId: "pi-5", cwd: "/tmp/proj", mcpServers: [] });
    expect(response.error).toBeUndefined();
    expect(spy.loaded).toEqual([{ sessionId: "pi-5", cwd: "/tmp/proj" }]);
    const updates = client
      .drained()
      .filter((message) => message.method === "session/update")
      .filter(
        (message) =>
          ((message.params as Record<string, unknown>).update as Record<string, unknown>).sessionUpdate !==
          "available_commands_update",
      );
    expect(updates).toHaveLength(2);
    expect(
      ((updates[0].params as Record<string, unknown>).update as Record<string, unknown>).sessionUpdate,
    ).toBe("user_message_chunk");
    // After the replay, the session answers prompts.
    const prompt = await client.request("session/prompt", { sessionId: "pi-5", prompt: [{ type: "text", text: "next" }] });
    expect((prompt.result as Record<string, unknown>).stopReason).toBe("end_turn");
    await client.close();
  });

  test("a load after new replaces the session object and its updates still flow", async () => {
    // Regression: the load path used to keep the previous session's pump, so
    // the replaced object streamed nothing.
    const created = new FakeSession({ id: "pi-6", stop: "end_turn" });
    const loaded = new FakeSession({
      id: "pi-6",
      replay: [{ sessionUpdate: "user_message_chunk", content: { type: "text", text: "history" }, messageId: "e1" }],
      updates: [{ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "live" }, messageId: "m1" }],
    });
    let disposed = 0;
    created.dispose = () => {
      disposed += 1;
    };
    const ports: HarnessPorts = {
      createSession: async () => created,
      loadSession: async () => loaded,
    };
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    await client.request("session/load", { sessionId: "pi-6", cwd: "/tmp/proj", mcpServers: [] });
    expect(disposed).toBe(1);
    const response = await client.request("session/prompt", {
      sessionId: "pi-6",
      prompt: [{ type: "text", text: "again" }],
    });
    expect((response.result as Record<string, unknown>).stopReason).toBe("end_turn");
    const updates = client
      .drained()
      .filter((message) => message.method === "session/update")
      .filter(
        (message) =>
          ((message.params as Record<string, unknown>).update as Record<string, unknown>).sessionUpdate !==
          "available_commands_update",
      );
    // One replay chunk + one live chunk from the replaced object.
    expect(updates.length).toBe(2);
    expect(loaded.prompts).toEqual(["again"]);
    await client.close();
  });

  test("a failing port becomes a coded internal error, logged on stderr", async () => {
    const logs: string[] = [];
    const ports: HarnessPorts = {
      createSession: async () => {
        throw new Error("disk exploded");
      },
      loadSession: async () => {
        throw new Error("no such file");
      },
    };
    const client = startClient(ports);
    await client.request("initialize", INITIALIZE_PARAMS);
    const response = await client.request("session/new", { cwd: "/tmp/proj" });
    const error = response.error as { code: number; data: { code: string } };
    expect(error.code).toBe(-32603);
    expect(error.data.code).toBe("session_new_failed");
    expect(client.logs.some((line) => line.includes("disk exploded"))).toBe(true);
    await client.close();
  });

  test("session/new advertises config options, modes and its command table", async () => {
    const session = new FakeSession({ id: "pi-7" });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/new", { cwd: "/tmp/proj" });
    const result = response.result as Record<string, unknown>;
    const options = result.configOptions as Array<Record<string, unknown>>;
    expect(options.map((option) => option.id)).toEqual(["model"]);
    expect((result.modes as Record<string, unknown>).currentModeId).toBe("ask");
    const commands = client
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => ((message.params as Record<string, unknown>).update as Record<string, unknown>))
      .filter((update) => update.sessionUpdate === "available_commands_update");
    expect(commands).toHaveLength(1);
    expect((commands[0].availableCommands as Array<Record<string, unknown>>)[0].name).toBe("compact");
    await client.close();
  });

  test("the session banner follows the command table on new, load and resume", async () => {
    const banner: SessionBanner = {
      version: "1.5.0",
      model: { provider: "dashscope", id: "qwen3.7-max", name: "qwen3.7-max" },
      thinkingLevel: "high",
      skills: [{ name: "nsh", description: "the repo's own", source: "user" }],
      extensions: [{ name: "my-ext", path: "/x/my-ext.ts", source: "project" }],
      warnings: [{ severity: "warning", source: "skill", message: "no SKILL.md frontmatter" }],
    };
    const newClient = startClient(fakePorts([new FakeSession({ id: "pi-b1", banner })]).ports);
    await initialize(newClient);
    await newClient.request("session/new", { cwd: "/tmp/proj" });
    const intro = newClient
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => (message.params as Record<string, unknown>).update as Record<string, unknown>);
    // Order: the command table first, the banner right after it.
    expect(intro.map((update) => update.sessionUpdate)).toEqual(["available_commands_update", "_zu/session_banner"]);
    expect(intro[1].banner).toEqual(banner);
    await newClient.close();

    const loadClient = startClient(fakePorts([new FakeSession({ id: "pi-b2", banner })]).ports);
    await initialize(loadClient);
    await loadClient.request("session/load", { sessionId: "pi-b2", cwd: "/tmp/proj" });
    const loadIntro = loadClient
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => (message.params as Record<string, unknown>).update as Record<string, unknown>);
    expect(loadIntro.at(-1)?.sessionUpdate).toBe("_zu/session_banner");
    await loadClient.close();

    const resumeClient = startClient(fakePorts([new FakeSession({ id: "pi-b3", banner })]).ports);
    await initialize(resumeClient);
    await resumeClient.request("session/resume", { sessionId: "pi-b3", cwd: "/tmp/proj" });
    const resumeIntro = resumeClient
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => (message.params as Record<string, unknown>).update as Record<string, unknown>);
    expect(resumeIntro.map((update) => update.sessionUpdate)).toEqual(["available_commands_update", "_zu/session_banner"]);
    await resumeClient.close();
  });

  test("a session without banner facts sends no banner notification", async () => {
    const session = new FakeSession({ id: "pi-b4", banner: null });
    const client = startClient(fakePorts([session]).ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const updates = client
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => (message.params as Record<string, unknown>).update as Record<string, unknown>);
    expect(updates.map((update) => update.sessionUpdate)).toEqual(["available_commands_update"]);
    // The same holds when the session has no banner method at all (the
    // plain FakeSession above); a fake stays unaffected by the announce.
    await client.close();
  });

  test("set_config_option and set_mode reach the session and answer fresh state", async () => {
    const session = new FakeSession({ id: "pi-8" });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const config = await client.request("session/set_config_option", {
      sessionId: "pi-8",
      configId: "model",
      type: "select",
      value: "p/other",
    });
    expect(config.error).toBeUndefined();
    expect(session.configChanges).toEqual([{ configId: "model", value: "p/other" }]);
    const options = (config.result as Record<string, unknown>).configOptions as Array<Record<string, unknown>>;
    expect(options[0].currentValue).toBe("p/other");
    // The thought level rides the same method (issue #54: the harness does
    // not second-guess the value — pi clamps it to the model's ladder).
    const thought = await client.request("session/set_config_option", {
      sessionId: "pi-8",
      configId: "thought_level",
      type: "select",
      value: "max",
    });
    expect(thought.error).toBeUndefined();
    expect(session.configChanges.at(-1)).toEqual({ configId: "thought_level", value: "max" });
    const mode = await client.request("session/set_mode", { sessionId: "pi-8", modeId: "auto" });
    expect(mode.error).toBeUndefined();
    expect(session.modeChanges).toEqual(["auto"]);
    const update = client
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => ((message.params as Record<string, unknown>).update as Record<string, unknown>))
      .find((entry) => entry.sessionUpdate === "current_mode_update");
    expect(update?.currentModeId).toBe("auto");
    await client.close();
  });

  test("session/resume reattaches without replaying history", async () => {
    const session = new FakeSession({
      id: "pi-9",
      replay: [{ sessionUpdate: "user_message_chunk", content: { type: "text", text: "old" }, messageId: "e1" }],
    });
    const { ports, spy } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    const response = await client.request("session/resume", { sessionId: "pi-9", cwd: "/tmp/proj" });
    expect(response.error).toBeUndefined();
    expect(spy.resumed).toEqual([{ sessionId: "pi-9", cwd: "/tmp/proj" }]);
    const replay = client
      .drained()
      .filter((message) => message.method === "session/update")
      .map((message) => ((message.params as Record<string, unknown>).update as Record<string, unknown>))
      .filter((update) => update.sessionUpdate !== "available_commands_update");
    expect(replay).toEqual([]);
    const prompt = await client.request("session/prompt", { sessionId: "pi-9", prompt: [{ type: "text", text: "go" }] });
    expect((prompt.result as Record<string, unknown>).stopReason).toBe("end_turn");
    await client.close();
  });

  test("session/close aborts, disposes and forgets the session", async () => {
    const session = new FakeSession({ id: "pi-10" });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const closed = await client.request("session/close", { sessionId: "pi-10" });
    expect(closed.error).toBeUndefined();
    expect(session.aborted).toBe(1);
    expect(session.disposed).toBe(1);
    const after = await client.request("session/prompt", { sessionId: "pi-10", prompt: [{ type: "text", text: "hi" }] });
    expect((after.error as { data: { code: string } }).data.code).toBe("session_not_found");
    await client.close();
  });

  test("a permission request round-trips through the client", async () => {
    const session = new FakeSession({ id: "pi-11", permission: true });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const promptId = await client.requestNoWait("session/prompt", {
      sessionId: "pi-11",
      prompt: [{ type: "text", text: "delete it" }],
    });
    const ask = await nextAgentRequest(client, "session/request_permission");
    expect((ask.params.toolCall as Record<string, unknown>).title).toBe("run the dangerous thing");
    const options = ask.params.options as Array<Record<string, unknown>>;
    expect(options.map((option) => option.optionId)).toEqual(["allow_once", "reject"]);
    await client.send({
      jsonrpc: "2.0",
      id: ask.id,
      result: { outcome: { outcome: "selected", optionId: "allow_once" } },
    });
    const response = await client.waitResponse(promptId);
    expect((response.result as Record<string, unknown>).stopReason).toBe("end_turn");
    expect(session.permissionAnswers).toEqual([
      { outcome: { outcome: "selected", optionId: "allow_once" } },
    ]);
    await client.close();
  });

  test("an elicitation round-trips through the client", async () => {
    const session = new FakeSession({ id: "pi-12", elicit: true });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const promptId = await client.requestNoWait("session/prompt", {
      sessionId: "pi-12",
      prompt: [{ type: "text", text: "deploy" }],
    });
    const ask = await nextAgentRequest(client, "elicitation/create");
    expect(ask.params.message).toBe("Which environment?");
    await client.send({
      jsonrpc: "2.0",
      id: ask.id,
      result: { action: "accept", content: { environment: "staging" } },
    });
    const response = await client.waitResponse(promptId);
    expect((response.result as Record<string, unknown>).stopReason).toBe("end_turn");
    expect(session.elicitationAnswers).toEqual([
      { action: "accept", content: { environment: "staging" } },
    ]);
    await client.close();
  });

  test("the _zu/subagents methods delegate to the session surface", async () => {
    const run = { id: "run-1", agent: "worker", status: "done" };
    const session = new FakeSession({
      id: "pi-13",
      subagents: {
        list: (filter) => [{ ...run, filter }],
        transcript: (runId) => ({ runId, rows: [] }),
        dispatch: (agent, task) => ({ id: "run-2", agent, task }),
        abort: async () => {},
      },
    });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const list = await client.request("_zu/subagents/list", { sessionId: "pi-13" });
    expect(((list.result as Record<string, unknown>).runs as Array<Record<string, unknown>>)[0].filter).toEqual({
      captainId: "pi-13",
    });
    const transcript = await client.request("_zu/subagents/transcript", { sessionId: "pi-13", runId: "run-1" });
    expect((transcript.result as Record<string, unknown>).runId).toBe("run-1");
    const dispatch = await client.request("_zu/subagents/dispatch", {
      sessionId: "pi-13",
      agent: "worker",
      task: "look",
    });
    expect((dispatch.result as Record<string, unknown>).run).toEqual({
      id: "run-2",
      agent: "worker",
      task: "look",
    });
    const abort = await client.request("_zu/subagents/abort", { sessionId: "pi-13", runId: "run-2" });
    expect(abort.error).toBeUndefined();
    await client.close();
  });

  test("the _zu/ask methods delegate to the mailbox surface", async () => {
    const answers: HarnessSession["answers"] = {
      pending: () => [{ toolCallId: "tc-9", args: { question: "which?" } }],
      answer: async (toolCallId, response) => {
        return toolCallId === "tc-9" ? { delivered: "late" } : { delivered: "live" };
      },
    };
    const session = new FakeSession({ id: "pi-14", answers });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const pending = await client.request("_zu/ask/pending", { sessionId: "pi-14" });
    expect((pending.result as Record<string, unknown>).asks).toEqual([
      { toolCallId: "tc-9", args: { question: "which?" } },
    ]);
    const late = await client.request("_zu/ask/answer", { sessionId: "pi-14", toolCallId: "tc-9", response: "sqlite" });
    expect((late.result as Record<string, unknown>).delivered).toBe("late");
    await client.close();
  });

  test("_zu/ask/answer without a mailbox answers an empty result; an unknown ask is a coded error", async () => {
    const session = new FakeSession({ id: "pi-15" });
    const { ports } = fakePorts([session]);
    const client = startClient(ports);
    await initialize(client);
    await client.request("session/new", { cwd: "/tmp/proj" });
    const empty = await client.request("_zu/ask/answer", { sessionId: "pi-15", toolCallId: "tc-x", response: "y" });
    expect(empty.error).toBeUndefined();
    expect(empty.result).toEqual({});
    const asks: HarnessSession["answers"] = {
      pending: () => [],
      answer: async () => ({ delivered: "none" }),
    };
    const knowing = new FakeSession({ id: "pi-16", answers: asks });
    const ports2 = fakePorts([knowing]).ports;
    const client2 = startClient(ports2);
    await initialize(client2);
    await client2.request("session/new", { cwd: "/tmp/proj" });
    const missing = await client2.request("_zu/ask/answer", {
      sessionId: "pi-16",
      toolCallId: "tc-gone",
      response: "y",
    });
    expect((missing.error as Record<string, unknown>).message).toContain("tc-gone");
    const data = (missing.error as Record<string, unknown>).data as Record<string, unknown>;
    expect(data.code).toBe("unknown_ask");
    await client.close();
    await client2.close();
  });

  test("the _zu/todos methods share the tools' store through the wire", async () => {
    const root = mkdtempSync(join(tmpdir(), "zu-todos-wire-"));
    try {
      const todos: HarnessSession["todos"] = {
        enabled: true,
        list: () => readPlan("wire-1", root),
        write: (op) => applyTodoOp("wire-1", root, op),
      };
      const session = new FakeSession({ id: "pi-17", todos });
      const { ports } = fakePorts([session]);
      const client = startClient(ports);
      await initialize(client);
      await client.request("session/new", { cwd: "/tmp/proj" });

      // A session without a plan reads as empty, not as an error.
      const empty = await client.request("_zu/todos/list", { sessionId: "pi-17" });
      expect((empty.result as Record<string, unknown>).plan).toEqual(EMPTY_PLAN);
      expect((empty.result as Record<string, unknown>).enabled).toBe(true);

      // A panel add rides the write; the next list sees the same file.
      const written = await client.request("_zu/todos/write", {
        sessionId: "pi-17",
        op: { kind: "add", title: "From the panel", origin: "user" },
      });
      const plan = (written.result as Record<string, unknown>).plan as { todos: Array<{ id: string; title: string }> };
      expect(plan.todos).toHaveLength(1);
      expect(plan.todos[0]).toMatchObject({ title: "From the panel", origin: "user" });
      const listed = await client.request("_zu/todos/list", { sessionId: "pi-17" });
      expect((listed.result as Record<string, unknown>).plan).toEqual(plan);

      // A refusal is a coded error whose data.code names the store's reason.
      const bad = await client.request("_zu/todos/write", {
        sessionId: "pi-17",
        op: { kind: "promote", id: "t-404" },
      });
      const badData = (bad.error as Record<string, unknown>).data as Record<string, unknown>;
      expect(badData.code).toBe("unknown-id");

      // No surface: list is empty, write refuses with its own code.
      const bare = new FakeSession({ id: "pi-18" });
      const ports2 = fakePorts([bare]).ports;
      const client2 = startClient(ports2);
      await initialize(client2);
      await client2.request("session/new", { cwd: "/tmp/proj" });
      const bareList = await client2.request("_zu/todos/list", { sessionId: "pi-18" });
      expect((bareList.result as Record<string, unknown>).plan).toEqual(EMPTY_PLAN);
      expect((bareList.result as Record<string, unknown>).enabled).toBe(false);
      const bareWrite = await client2.request("_zu/todos/write", { sessionId: "pi-18", op: { kind: "add", title: "x" } });
      expect(((bareWrite.error as Record<string, unknown>).data as Record<string, unknown>).code).toBe(
        "todos_unavailable",
      );
      await client.close();
      await client2.close();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("captain extension assembly", () => {
  /** Fake ExtensionAPI that records `on` registrations per event name. */
  function fakePi() {
    const registered = new Map<string, Array<(...args: unknown[]) => unknown>>();
    const pi = {
      on(event: string, handler: (...args: unknown[]) => unknown) {
        const list = registered.get(event) ?? [];
        list.push(handler);
        registered.set(event, list);
        return () => {};
      },
    };
    return { pi: pi as unknown as ExtensionAPI, registered };
  }

  test("factory list binds gate, the two prompt sections, the artifacts guard, then dispatch", () => {
    const noop = () => {};
    const gate = { extension: () => () => {} }; // Pick<PermissionGate, "extension">
    const subagentManager = new SubagentManager({ settledRuns: () => [] } as never);
    const factories = hostExtensionFactories(gate, subagentManager, "pi-1", noop);
    expect(factories).toHaveLength(5);

    // Factories 1–2 are the prompt-section ones: each registers exactly one
    // `before_agent_start` handler, which writes its named rule and returns
    // nothing (a returned systemPrompt would bust the prefix cache).
    for (const [index, section, rule] of [
      [1, "zu-dispatch", DISPATCH_RULE],
      [2, "zu-artifacts", ARTIFACTS_RULE],
    ] as const) {
      const { pi, registered } = fakePi();
      (factories[index] as (pi: ExtensionAPI) => void)(pi);
      expect([...registered.keys()]).toEqual(["before_agent_start"]);
      const sections: Record<string, string> = {};
      const result = (registered.get("before_agent_start")![0] as (e: unknown) => unknown)({
        systemPromptOptions: { sections },
      });
      expect(sections[section]).toBe(rule);
      expect(result).toBeUndefined();
    }

    // Factory 3 is the artifacts guard: exactly one `tool_call` handler.
    const guard = fakePi();
    (factories[3] as (pi: ExtensionAPI) => void)(guard.pi);
    expect([...guard.registered.keys()]).toEqual(["tool_call"]);
    expect(guard.registered.get("tool_call")).toHaveLength(1);
  });
});
