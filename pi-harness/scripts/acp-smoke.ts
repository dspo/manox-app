/**
 * ACP smoke for the compiled distro: spawns `dist/pi-acp` and drives it over
 * stdio ndjson like a real client would.
 *
 * Default (offline): initialize + session/new, framing probes (split frames,
 * pipelined ids, malformed line, batch array) and a clean stdin-close exit.
 * No model call happens.
 *
 * ACP_SMOKE_LIVE=1: additionally runs a real prompt turn against the model
 * the user's ~/.pi is configured with — asserting the streamed updates, the
 * `end_turn` stopReason, the new session file under ~/.pi/agent/sessions/, a
 * `session/load` replay of that same session, and a mid-turn
 * `session/cancel` answering `cancelled`.
 *
 * Exit code 0 = all assertions held.
 */

import { existsSync, mkdtempSync, readdirSync, rmSync, statSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

const DIST = join(import.meta.dir, "..", "dist", "pi-acp");
const LIVE = process.env.ACP_SMOKE_LIVE === "1";
const decoder = new TextDecoder();
const encoder = new TextEncoder();

if (!existsSync(DIST)) {
  console.error(`dist/pi-acp not found; run \`bun run build\` first`);
  process.exit(2);
}

class Failure extends Error {}

function assert(condition: unknown, message: string): void {
  if (!condition) throw new Failure(message);
}

// ------------------------------------------------------------------ client

interface Pending {
  resolve: (message: Record<string, any>) => void;
  reject: (error: Error) => void;
}

const agent = Bun.spawn([DIST], {
  stdin: "pipe",
  stdout: "pipe",
  stderr: "pipe",
});

const stdin = agent.stdin!;
const stdout = agent.stdout!;
let stderrTail = "";

// Declared before the reader tasks that fill them: the closures run later,
// but the temporal-dead-zone read is textual.
const pendingByResponseId = new Map<string, Pending>();
const notifications: Array<Record<string, any>> = [];
let nextId = 0;

void (async () => {
  const reader = stdout.getReader();
  let buffer = "";
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let newline = buffer.indexOf("\n");
    while (newline >= 0) {
      const line = buffer.slice(0, newline).trim();
      buffer = buffer.slice(newline + 1);
      if (line.length > 0) {
        let message: Record<string, any>;
        try {
          message = JSON.parse(line);
        } catch {
          console.error(`unparseable frame from agent: ${line.slice(0, 200)}`);
          newline = buffer.indexOf("\n");
          continue;
        }
        if (
          typeof message.method === "string" &&
          message.id !== undefined &&
          message.id !== null &&
          !("result" in message) &&
          !("error" in message)
        ) {
          // The distro may ask for project trust while session/new is in
          // flight. This smoke has no UI; declining keeps it headless.
          if (message.method === "elicitation/create") {
            stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result: { action: "decline" } })}\n`);
          } else {
            stdin.write(
              `${JSON.stringify({ jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "not served by smoke" } })}\n`,
            );
          }
          newline = buffer.indexOf("\n");
          continue;
        }
        const pending = pendingByResponseId.get(String(message.id));
        if (pending !== undefined && ("result" in message || "error" in message)) {
          pendingByResponseId.delete(String(message.id));
          pending.resolve(message);
        } else {
          notifications.push(message);
        }
      }
      newline = buffer.indexOf("\n");
    }
  }
})();

void (async () => {
  const reader = agent.stderr!.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    stderrTail = (stderrTail + decoder.decode(value, { stream: true })).slice(-4000);
  }
})();

function rawWrite(text: string): Promise<void> {
  stdin.write(text);
  return Promise.resolve();
}

async function send(message: unknown): Promise<void> {
  await rawWrite(`${JSON.stringify(message)}\n`);
}

async function request(method: string, params?: unknown, timeoutMs = 90_000): Promise<Record<string, any>> {
  const id = ++nextId;
  const response = new Promise<Record<string, any>>((resolve, reject) => {
    pendingByResponseId.set(String(id), { resolve, reject });
    setTimeout(() => {
      if (pendingByResponseId.delete(String(id))) reject(new Failure(`timeout on ${method} (id ${id})`));
    }, timeoutMs).unref();
  });
  await send({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
  return response;
}

// ----------------------------------------------------------------- checks

const cwd = mkdtempSync(join(tmpdir(), "pi-acp-smoke-"));

try {
  // 1. initialize — capability surface.
  const init = await request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
  assert(init.result, `initialize failed: ${JSON.stringify(init.error)}`);
  assert(init.result.protocolVersion === 1, `protocolVersion should be 1, got ${init.result.protocolVersion}`);
  assert(init.result.agentCapabilities?.loadSession === true, "agentCapabilities.loadSession must be true");
  assert(init.result.agentCapabilities?.promptCapabilities?.image === false, "promptCapabilities.image must be false");
  assert(Array.isArray(init.result.authMethods) && init.result.authMethods.length === 0, "authMethods must be []");
  console.log("initialize ok:", JSON.stringify(init.result.agentInfo));

  // 2. id pairing with two pipelined requests in one write.
  const batch = [
    JSON.stringify({ jsonrpc: "2.0", id: 901, method: "probe/missing" }),
    JSON.stringify({ jsonrpc: "2.0", id: 902, method: "probe/also_missing" }),
  ].join("\n");
  await rawWrite(`${batch}\n`);
  const firstMissing = await waitForResponse(901);
  const secondMissing = await waitForResponse(902);
  assert(firstMissing.error?.code === -32601, `unknown method should be -32601, got ${JSON.stringify(firstMissing.error)}`);
  assert(firstMissing.error?.data?.code === "method_not_found", "unknown method must carry data.code");
  assert(secondMissing.id === 902, `pipelined id pairing broken: ${JSON.stringify(secondMissing)}`);
  console.log("id pairing ok (pipelined, error path)");

  // 3. framing: a frame cut into three arbitrary byte chunks still parses.
  const frame = `${JSON.stringify({ jsonrpc: "2.0", id: 903, method: "probe/split" })}\n`;
  stdin.write(frame.slice(0, 7));
  stdin.write(frame.slice(7, 33));
  stdin.write(frame.slice(33));
  const split = await waitForResponse(903);
  assert(split.error?.code === -32601, "split frame should still dispatch");
  console.log("split-frame framing ok");

  // 4. session/new (offline): mcpServers omitted on purpose.
  const created = await request("session/new", { cwd });
  assert(created.result?.sessionId, `session/new failed: ${JSON.stringify(created.error)}`);
  const sessionId = String(created.result.sessionId);
  assert(Array.isArray(created.result.configOptions), "session/new must return configOptions");
  assert(created.result.modes?.availableModes?.length > 0, "session/new must return the approval modes");
  const announced = notifications.some(
    (message) =>
      message.method === "session/update" &&
      message.params?.update?.sessionUpdate === "available_commands_update",
  );
  assert(announced, "session/new must announce available_commands_update");
  console.log(`session/new ok: ${sessionId} (${created.result.configOptions.length} config option(s))`);
  await send({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId } });
  console.log("session/cancel accepted (notification, no reply)");

  // 5. protocol garbage: malformed line, then a batch array.
  await rawWrite("{definitely not json}\n");
  const parseError = await nextRawMessage();
  assert(parseError.id === null, `malformed line should answer id null, got ${JSON.stringify(parseError)}`);
  assert(parseError.error?.code === -32700, `malformed line should be -32700, got ${JSON.stringify(parseError.error)}`);
  await rawWrite('[{"jsonrpc":"2.0","id":905,"method":"initialize"}]\n');
  const batchError = await nextRawMessage();
  assert(batchError.error?.code === -32600, `batch array should be -32600, got ${JSON.stringify(batchError.error)}`);
  console.log("protocol garbage handled (-32700, -32600)");

  if (!LIVE) {
    console.log("offline smoke ok (set ACP_SMOKE_LIVE=1 for the real turn)");
  } else {
    // 6. live prompt turn: real model, streamed updates, settle.
    const prompt = await request(
      "session/prompt",
      { sessionId, prompt: [{ type: "text", text: "Reply with exactly: ok" }] },
      180_000,
    );
    assert(prompt.result, `session/prompt failed: ${JSON.stringify(prompt.error)}`);
    assert(prompt.result.stopReason === "end_turn", `stopReason should be end_turn, got ${prompt.result.stopReason}`);
    // 6a. A live turn announces the user's own message and settles the
    // reply: pi persists messages at message_end and never emits
    // entry_appended for them, so the distro reads the entry back. A
    // regression here leaves the client's optimistic row re-keyless and the
    // reply an open draft (and a prompt sent while that draft is open lands
    // above the answer it replies to).
    const liveKinds = new Set<string>();
    let updates = 0;
    for (let index = notifications.length - 1; index >= 0; index -= 1) {
      const message = notifications[index]!;
      if (message.method !== "session/update" || message.params?.sessionId !== sessionId) continue;
      notifications.splice(index, 1);
      updates += 1;
      const update = message.params?.update as Record<string, any> | undefined;
      if (!update) continue;
      if (update.sessionUpdate === "user_message_chunk") liveKinds.add("user");
      if (update.sessionUpdate === "agent_message_chunk" && update._meta?.pi?.settled === true) {
        liveKinds.add("settle");
      }
    }
    assert(updates > 0, "the live turn should have streamed at least one session/update");
    assert(liveKinds.has("user"), "a live turn must send the user's message chunk (user_entry_chunks)");
    assert(liveKinds.has("settle"), "a live turn must settle the assistant message (settle marker)");
    console.log(`live prompt ok: stopReason=end_turn, updates=${updates} (user chunk + settle)`);

    // 7. the session file exists under ~/.pi/agent/sessions/.
    const sessionFile = findSessionFile(sessionId);
    assert(sessionFile !== undefined, `no session file for ${sessionId} under ${join(homedir(), ".pi", "agent", "sessions")}`);
    console.log("session file ok:", sessionFile);

    // 8. session/load replays the history as updates before the response.
    const load = await request("session/load", { sessionId, cwd, mcpServers: [] }, 90_000);
    assert(load.result !== undefined, `session/load failed: ${JSON.stringify(load.error)}`);
    let sawUser = false;
    let sawAgent = false;
    for (let _ = 0; _ < updates + 2; _ += 1) {
      const message = notifications.shift();
      if (!message || message.method !== "session/update") break;
      const kind = message.params?.update?.sessionUpdate;
      if (kind === "user_message_chunk") sawUser = true;
      if (kind === "agent_message_chunk") sawAgent = true;
    }
    assert(sawUser, "load replay must include the user message");
    assert(sawAgent, "load replay must include the agent message");
    console.log("session/load replay ok (user + agent chunks)");

    // 9. cancel mid-turn answers cancelled.
    const cancelProbeId = ++nextId;
    const cancelResponse = new Promise<Record<string, any>>((resolve, reject) => {
      pendingByResponseId.set(String(cancelProbeId), { resolve, reject });
      setTimeout(() => {
        if (pendingByResponseId.delete(String(cancelProbeId))) reject(new Failure("timeout on the cancel probe"));
      }, 150_000).unref();
    });
    await send({
      jsonrpc: "2.0",
      id: cancelProbeId,
      method: "session/prompt",
      params: { sessionId, prompt: [{ type: "text", text: "Count slowly from 1 to 200, one number per line." }] },
    });
    // Wait until the turn is under way: its first streamed update. If the
    // turn settles (or errors) first, the window is gone — say so with the
    // response in the message. Providers with retry backoff can take a
    // while before the first chunk, hence the generous window.
    const settleDeadline = Date.now() + 120_000;
    let firstUpdateSeen = false;
    while (Date.now() < settleDeadline) {
      const updateIndex = notifications.findIndex(
        (message) => message.method === "session/update" && message.params?.sessionId === sessionId,
      );
      if (updateIndex >= 0) {
        firstUpdateSeen = true;
        break;
      }
      const settled = pendingByResponseId.has(String(cancelProbeId)) === false;
      if (settled) break;
      await new Promise((resolve) => setTimeout(resolve, 30));
    }
    if (!firstUpdateSeen) {
      const settled = await Promise.race([
        cancelResponse,
        new Promise<null>((resolve) => setTimeout(() => resolve(null), 1_000)),
      ]);
      throw new Failure(
        `cancel probe: no update before the turn settled: ${JSON.stringify(settled ?? "response pending")}`,
      );
    }
    await send({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId } });
    const cancelled = await cancelResponse;
    assert(
      cancelled.result?.stopReason === "cancelled",
      `cancel should settle cancelled, got ${JSON.stringify(cancelled.result ?? cancelled.error)}`,
    );
    console.log("live cancel ok: stopReason=cancelled");
    console.log("LIVE SMOKE OK");
  }

  // 10. graceful exit: closing stdin flushes and ends the process.
  stdin.end();
  const code = await agent.exited;
  assert(code === 0, `agent should exit 0 on stdin close, got ${code}`);
  console.log("graceful exit ok (stdin close, code 0)");
  console.log("SMOKE PASSED");
} catch (error) {
  console.error(`SMOKE FAILED: ${error instanceof Error ? error.message : String(error)}`);
  console.error(`--- stderr tail ---\n${stderrTail}`);
  try {
    stdin.end();
  } catch {
    // Already gone.
  }
  await Promise.race([agent.exited, new Promise((resolve) => setTimeout(resolve, 3000))]);
  agent.kill();
  process.exitCode = 1;
} finally {
  try {
    rmSync(cwd, { recursive: true, force: true });
  } catch {
    // A temp dir that would not go away is not a smoke failure.
  }
}

/** Frames sent outside `request()` are polled out of the notification sink. */
async function waitForResponse(id: number, timeoutMs = 10_000): Promise<Record<string, any>> {
  const deadline = Date.now() + timeoutMs;
  while (true) {
    const index = notifications.findIndex(
      (message) => message.id === id && ("result" in message || "error" in message),
    );
    if (index >= 0) {
      return notifications.splice(index, 1)[0]!;
    }
    if (Date.now() > deadline) throw new Failure(`timeout waiting for response id ${id}`);
    await new Promise((resolve) => setTimeout(resolve, 15));
  }
}

async function nextRawMessage(timeoutMs = 10_000): Promise<Record<string, any>> {
  const deadline = Date.now() + timeoutMs;
  while (true) {
    // Session updates (the command table, tool streaming) are not replies;
    // a framing probe wants the next protocol envelope.
    const index = notifications.findIndex(
      (message) => !(message.method === "session/update" && message.id === undefined),
    );
    if (index >= 0) return notifications.splice(index, 1)[0]!;
    if (Date.now() > deadline) throw new Failure("timeout waiting for any protocol frame");
    await new Promise((resolve) => setTimeout(resolve, 15));
  }
}

/** Finds the session's jsonl file under ~/.pi/agent/sessions/ by id. */
function findSessionFile(sessionId: string): string | undefined {
  const root = join(homedir(), ".pi", "agent", "sessions");
  if (!existsSync(root)) return undefined;
  const suffix = `_${sessionId}.jsonl`;
  const visit = (dir: string, depth: number): string | undefined => {
    if (depth > 3) return undefined;
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return undefined;
    }
    for (const entry of entries) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        const found = visit(path, depth + 1);
        if (found) return found;
      } else if (entry.name.endsWith(suffix) && statSync(path).size > 0) {
        return path;
      }
    }
    return undefined;
  };
  return visit(root, 0);
}
