/**
 * Live acceptance for the ACP session runtime (docs/pi-acp-IMPLEMENTATION.md
 * §11.2). Drives the compiled distro against the model the user's ~/.pi is
 * configured with — a real turn, a real session file, real approvals.
 *
 * Run:
 *   bun run scripts/acp-live-acceptance.ts
 *
 * Exit code 0 only when every executed check holds. Checks that cannot run on
 * this machine (e.g. a single configured model) are reported as SKIP, not
 * PASS. Nothing here is imported by the app or the distro.
 */

import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { FileSink } from "bun";

const DIST = join(import.meta.dir, "..", "dist", "pi-acp");
const CWD = join(import.meta.dir, "..", "..");
const MCP_ECHO = join(import.meta.dir, "fixtures", "mcp-echo.ts");

if (!existsSync(DIST)) {
  console.error("dist/pi-acp not found; run `bun run build` first");
  process.exit(2);
}

class Failure extends Error {}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Failure(message);
}

type PermissionPolicy = "allow" | "reject";

interface PermissionAsk {
  toolCall?: { title?: string; kind?: string };
  options?: Array<{ optionId: string; name?: string; kind?: string }>;
}

interface Turn {
  stopReason: string;
  updates: Array<Record<string, any>>;
  permissions: PermissionAsk[];
  elicitations: Array<Record<string, any>>;
  startedAt: number;
  firstTokenAt?: number;
}

/** One distro process and the ACP client over its stdio. */
class LiveClient {
  private readonly process: ReturnType<typeof Bun.spawn>;
  private readonly pending = new Map<string, { resolve: (m: any) => void; reject: (e: Error) => void }>();
  private readonly queue: Array<Record<string, any>> = [];
  private nextId = 0;
  permissionPolicy: PermissionPolicy = "allow";
  elicitationPolicy: "accept" | "decline" = "decline";
  /** Servers passed on this client's session/new. */
  mcpServers: Array<Record<string, unknown>> = [];
  elicitationContent: Record<string, unknown> = {};
  readonly turns: Turn[] = [];
  private turn: Turn | null = null;

  /** `stdin: "pipe"` narrows to a sink at runtime; Bun's type unions it with
   * the raw fd number. */
  private get input(): FileSink {
    return this.process.stdin as unknown as FileSink;
  }

  constructor(options: { env?: Record<string, string> } = {}) {
    this.process = Bun.spawn([DIST], {
      cwd: CWD,
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      env: { ...(process.env as Record<string, string>), ...(options.env ?? {}) },
    });
    void this.process.exited.then(() => {
      for (const [, entry] of this.pending) entry.reject(new Failure("the harness process exited"));
      this.pending.clear();
    });
    this.read();
    this.readStderr();
  }

  private async read(): Promise<void> {
    const reader = (this.process.stdout as unknown as ReadableStream<Uint8Array>).getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (line) this.handle(line);
        newline = buffer.indexOf("\n");
      }
    }
  }

  private async readStderr(): Promise<void> {
    const reader = (this.process.stderr as unknown as ReadableStream<Uint8Array>).getReader();
    const decoder = new TextDecoder();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      const text = decoder.decode(value, { stream: true }).trim();
      if (text) console.error(`[pi-acp stderr] ${text}`);
    }
  }

  private handle(line: string): void {
    let message: Record<string, any>;
    try {
      message = JSON.parse(line);
    } catch {
      console.error(`unparseable frame: ${line.slice(0, 160)}`);
      return;
    }
    const hasId = message.id !== undefined && message.id !== null;
    if (hasId && (message.result !== undefined || message.error !== undefined)) {
      const entry = this.pending.get(String(message.id));
      if (!entry) return;
      this.pending.delete(String(message.id));
      if (message.error) entry.reject(new Failure(`${message.error.message} (${message.error.data?.code ?? "no code"})`));
      else entry.resolve(message.result);
      return;
    }
    if (typeof message.method === "string" && hasId) {
      this.answerRequest(message);
      return;
    }
    if (message.method === "session/update") this.observeUpdate(message.params?.update);
  }

  private answerRequest(message: Record<string, any>): void {
    if (message.method === "session/request_permission") {
      const ask = (message.params ?? {}) as PermissionAsk;
      this.turn?.permissions.push(ask);
      const optionId = this.permissionPolicy === "allow" ? "allow_once" : "reject";
      this.reply(message.id, { outcome: { outcome: "selected", optionId } });
      return;
    }
    if (message.method === "elicitation/create") {
      const params = message.params ?? {};
      this.turn?.elicitations.push(params);
      if (this.elicitationPolicy === "accept") {
        const properties = params.requestedSchema?.properties ?? {};
        const content: Record<string, unknown> = { ...this.elicitationContent };
        if ("confirmed" in properties && content.confirmed === undefined) content.confirmed = true;
        if ("choice" in properties && content.choice === undefined) content.choice = "beta";
        if ("value" in properties && content.value === undefined) content.value = "live-input";
        this.reply(message.id, { action: "accept", content });
      } else {
        this.reply(message.id, { action: "decline" });
      }
      return;
    }
    this.replyError(message.id, -32601, `the acceptance client does not serve ${message.method}`);
  }

  private observeUpdate(update: Record<string, any> | undefined): void {
    if (!update) return;
    if (this.turn) {
      this.turn.updates.push(update);
      if (!this.turn.firstTokenAt && update.sessionUpdate === "agent_message_chunk") {
        this.turn.firstTokenAt = Date.now();
      }
    }
    if (update.sessionUpdate === "available_commands_update") {
      console.log(`  commands advertised: ${(update.availableCommands ?? []).length}`);
    }
  }

  reply(id: number | string, result: unknown): void {
    this.input.write(`${JSON.stringify({ jsonrpc: "2.0", id, result })}\n`);
  }

  private replyError(id: number | string, code: number, message: string): void {
    this.input.write(`${JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } })}\n`);
  }

  request<T = any>(method: string, params?: unknown, timeoutMs = 180_000): Promise<T> {
    const id = ++this.nextId;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(String(id));
        reject(new Failure(`${method} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(String(id), {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.input.write(
        `${JSON.stringify({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) })}\n`,
      );
    });
  }

  async turnPrompt(sessionId: string, text: string, timeoutMs = 180_000): Promise<Turn> {
    const turn: Turn = {
      stopReason: "",
      updates: [],
      permissions: [],
      elicitations: [],
      startedAt: Date.now(),
    };
    this.turn = turn;
    try {
      const result = await this.request<any>("session/prompt", {
        sessionId,
        prompt: [{ type: "text", text }],
      }, timeoutMs);
      turn.stopReason = String(result?.stopReason ?? "");
      this.turns.push(turn);
      return turn;
    } finally {
      this.turn = null;
    }
  }

  get pid(): number {
    return this.process.pid;
  }

  /** Simulates a crash, not a graceful close. */
  crash(): void {
    try {
      this.process.kill("SIGKILL");
    } catch {
      // Already gone.
    }
  }

  async close(): Promise<void> {
    try {
      this.input.end();
    } catch {
      // Already closed.
    }
    await Promise.race([this.process.exited, new Promise((resolve) => setTimeout(resolve, 3000))]);
    try {
      this.process.kill();
    } catch {
      // Already gone.
    }
  }
}

function configOf(options: any[], id: string): any | undefined {
  return options.find((option) => option.id === id);
}

function choice(options: any[], id: string): string | undefined {
  return configOf(options, id)?.currentValue;
}

function lastUsage(turn: Turn): { used: number; size: number } | undefined {
  for (let index = turn.updates.length - 1; index >= 0; index -= 1) {
    const update = turn.updates[index];
    if (update.sessionUpdate === "usage_update") {
      return { used: Number(update.used ?? 0), size: Number(update.size ?? 0) };
    }
  }
  return undefined;
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function rssMb(pid: number): string {
  try {
    const out = Bun.spawnSync(["ps", "-o", "rss=", "-p", String(pid)]).stdout.toString().trim();
    const kb = Number(out);
    return Number.isFinite(kb) && kb > 0 ? `${(kb / 1024).toFixed(1)} MB` : "unknown";
  } catch {
    return "unknown";
  }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function crashCheck(markerDir: string): Promise<void> {
  console.log("→ kill mid-turn → resume");
  let crash = new LiveClient();
  try {
    await crash.request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
    const created = await crash.request<any>("session/new", { cwd: CWD, mcpServers: [] }, 90_000);
    const id = String(created.sessionId);
    const crashedPid = crash.pid;
    const inFlight = crash.turnPrompt(id, "Write a 500-word essay about the sea.", 120_000);
    await sleep(800);
    crash.crash();
    const failure = await inFlight.catch((error) => error);
    assert(failure instanceof Error, "the killed turn did not reject");
    await sleep(300);
    assert(!isAlive(crashedPid), "the killed process is still alive");
    console.log(`  ok: the turn rejected after SIGKILL (${failure.message})`);

    crash = new LiveClient();
    await crash.request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
    await crash.request("session/resume", { id, sessionId: id, cwd: CWD, mcpServers: [] }, 90_000);
    const resumed = await crash.turnPrompt(id, "Reply with exactly: after-crash", 120_000);
    assert(resumed.stopReason === "end_turn", `resumed turn stopReason ${resumed.stopReason}`);
    console.log("  ok: the same session answered after resume");
  } finally {
    void markerDir;
    await crash.close();
  }
}

async function main(): Promise<void> {
  const markerDir = mkdtempSync(join(tmpdir(), "zu-live-"));
  if (process.env.LIVE_CRASH_ONLY === "1") {
    try {
      await crashCheck(markerDir);
      console.log("LIVE CRASH CHECK PASSED");
      return;
    } finally {
      rmSync(markerDir, { recursive: true, force: true });
    }
  }
  const allowMarker = join(markerDir, "allow.txt");
  const rejectMarker = join(markerDir, "reject.txt");
  let client = new LiveClient();
  let sessionId = "";
  let chosenModel: string | undefined;
  let chosenLevel: string | undefined;
  let toolModel: string | undefined;

  try {
    console.log("→ initialize + session/new");
    const coldStart = Date.now();
    const init = await client.request<any>("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
    assert(init?.protocolVersion === 1, "protocolVersion is not 1");
    client.mcpServers = [
      { name: "fixture", command: process.execPath, args: [MCP_ECHO] },
    ];
    const created = await client.request<any>("session/new", { cwd: CWD, mcpServers: client.mcpServers }, 90_000);
    sessionId = String(created?.sessionId ?? "");
    assert(sessionId, "session/new returned no sessionId");
    let options = created.configOptions ?? [];
    assert(configOf(options, "model"), "no model config option: this machine has no usable model");
    console.log(`  session ${sessionId}`);
    console.log(`  models: ${configOf(options, "model").options.length}, current ${choice(options, "model")}`);
    console.log(`  thought levels: ${(configOf(options, "thought_level")?.options ?? []).length}`);
    console.log(`  cold start: ${Date.now() - coldStart} ms, rss ${rssMb(client.pid)}`);

    console.log("→ baseline turn (real model)");
    const baseline = await client.turnPrompt(sessionId, "Reply with exactly: live-ok");
    assert(baseline.stopReason === "end_turn", `baseline stopReason ${baseline.stopReason}`);
    const baselineText = baseline.updates
      .filter((update) => update.sessionUpdate === "agent_message_chunk")
      .map((update) => update.content?.text ?? "")
      .join("");
    assert(baselineText.length > 0, "baseline turn streamed no text");
    console.log(
      `  ok: ${baselineText.trim().slice(0, 60)} (first token ${baseline.firstTokenAt ? baseline.firstTokenAt - baseline.startedAt : "?"} ms)`,
    );

    console.log("→ codemode from the user's settings.json");
    const codeTurn = await client.turnPrompt(
      sessionId,
      "Use the codemode tool to compute 6*7 and reply with just the result.",
      180_000,
    );
    assert(codeTurn.stopReason === "end_turn", `codemode turn stopReason ${codeTurn.stopReason}`);
    const called = codeTurn.updates
      .filter((update) => update.sessionUpdate === "tool_call")
      .map((update) => String(update.name ?? update.title ?? ""));
    assert(called.some((name) => name.toLowerCase().includes("codemode")), `codemode was not called; tools=${called.join(",")}`);
    const codeText = codeTurn.updates
      .filter((update) => update.sessionUpdate === "agent_message_chunk")
      .map((update) => update.content?.text ?? "")
      .join("");
    assert(codeText.includes("42"), `codemode answer was ${codeText.slice(0, 120)}`);
    console.log("  ok: codemode tool called and returned 42");

    console.log("→ ACP session/new mcpServers");
    const mcpTurn = await client.turnPrompt(
      sessionId,
      "Use the mcp__fixture__echo tool to echo exactly: mcp-live-ok. Use that tool and no other.",
      180_000,
    );
    assert(mcpTurn.stopReason === "end_turn", `MCP turn stopReason ${mcpTurn.stopReason}`);
    const mcpTools = mcpTurn.updates
      .filter((update) => update.sessionUpdate === "tool_call")
      .map((update) => String(update.name ?? update.title ?? ""));
    assert(mcpTools.some((name) => name.includes("fixture") || name.includes("echo")), `MCP tool not called; tools=${mcpTools.join(",")}`);
    const mcpText = mcpTurn.updates
      .filter((update) => update.sessionUpdate === "agent_message_chunk")
      .map((update) => update.content?.text ?? "")
      .join("");
    assert(mcpText.includes("mcp-live-ok"), `MCP answer was ${mcpText.slice(0, 120)}`);
    console.log("  ok: the ACP MCP server's echo tool ran");

    console.log("→ in-session model switch");
    const modelOption = configOf(options, "model");
    const otherModel = modelOption.options.map((option: any) => option.value).find((value: string) => value !== modelOption.currentValue);
    if (!otherModel) {
      console.log("  SKIP: only one model is available");
    } else {
      const switched = await client.request<any>(
        "session/set_config_option",
        { sessionId, configId: "model", type: "select", value: otherModel },
        60_000,
      );
      options = switched.configOptions ?? options;
      assert(choice(options, "model") === otherModel, `model did not switch: ${choice(options, "model")}`);
      chosenModel = otherModel;
      toolModel = otherModel;
      const afterModel = await client.turnPrompt(sessionId, "Reply with exactly: model-ok");
      assert(afterModel.stopReason === "end_turn", `switched-model turn stopReason ${afterModel.stopReason}`);
      console.log(`  ok: ${otherModel}`);
    }

    console.log("→ find a model that exposes thinking levels");
    let thought = configOf(options, "thought_level");
    if ((thought?.options?.length ?? 0) <= 1) {
      const restore = choice(options, "model");
      for (const candidate of modelOption.options.map((option: any) => option.value)) {
        if (candidate === choice(options, "model")) continue;
        try {
          const reply = await client.request<any>(
            "session/set_config_option",
            { sessionId, configId: "model", type: "select", value: candidate },
            30_000,
          );
          const levels = configOf(reply.configOptions ?? [], "thought_level");
          if ((levels?.options?.length ?? 0) > 1) {
            options = reply.configOptions;
            chosenModel = candidate;
            thought = levels;
            console.log(`  found: ${candidate} (${levels.options.length} levels)`);
            break;
          }
        } catch {
          // A provider this machine cannot select; keep looking.
        }
      }
      if ((thought?.options?.length ?? 0) <= 1 && restore) {
        const reply = await client.request<any>(
          "session/set_config_option",
          { sessionId, configId: "model", type: "select", value: restore },
          30_000,
        );
        options = reply.configOptions ?? options;
      }
    } else {
      console.log(`  current model exposes ${thought.options.length} levels`);
    }

    console.log("→ in-session thought-level switch");
    thought = configOf(options, "thought_level");
    const levels = thought?.options.map((option: any) => option.value) ?? [];
    const otherLevel =
      levels.find((value: string) => value !== thought?.currentValue && value !== "off") ??
      levels.find((value: string) => value !== thought?.currentValue);
    if (!thought || !otherLevel) {
      console.log("  SKIP: no alternative thinking level");
    } else {
      const switched = await client.request<any>(
        "session/set_config_option",
        { sessionId, configId: "thought_level", type: "select", value: otherLevel },
        60_000,
      );
      options = switched.configOptions ?? options;
      assert(choice(options, "thought_level") === otherLevel, `thought level did not switch: ${choice(options, "thought_level")}`);
      chosenLevel = otherLevel;
      const afterLevel = await client.turnPrompt(sessionId, "Reply with exactly: level-ok");
      assert(afterLevel.stopReason === "end_turn", `switched-level turn stopReason ${afterLevel.stopReason}`);
      console.log(`  ok: ${otherLevel}`);
    }


    console.log("→ resume after a new process");
    const beforeModel = choice(options, "model");
    const beforeLevel = choice(options, "thought_level");
    await client.request("session/close", { sessionId }, 30_000).catch(() => {});
    await client.close();
    client = new LiveClient();
    const reinit = await client.request<any>("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
    assert(reinit?.protocolVersion === 1, "reinitialize failed");
    const resumed = await client.request<any>("session/resume", { sessionId, cwd: CWD, mcpServers: [] }, 90_000);
    options = resumed.configOptions ?? [];
    console.log(`  resumed model ${choice(options, "model")} (was ${beforeModel}), level ${choice(options, "thought_level")} (was ${beforeLevel})`);
    if (beforeModel && choice(options, "model") !== beforeModel) {
      throw new Failure("model did not survive resume");
    }
    if (beforeLevel && choice(options, "thought_level") !== beforeLevel) {
      throw new Failure("thought level did not survive resume");
    }
    const resumedTurn = await client.turnPrompt(sessionId, "Reply with exactly: resume-ok");
    assert(resumedTurn.stopReason === "end_turn", `resumed turn stopReason ${resumedTurn.stopReason}`);
    console.log(
      `  ok: resumed session answers (first token ${resumedTurn.firstTokenAt ? resumedTurn.firstTokenAt - resumedTurn.startedAt : "?"} ms)`,
    );

    if (toolModel && choice(options, "model") !== toolModel) {
      console.log(`→ restore the tool model for approvals: ${toolModel}`);
      const reply = await client.request<any>(
        "session/set_config_option",
        { sessionId, configId: "model", type: "select", value: toolModel },
        30_000,
      );
      options = reply.configOptions ?? options;
    }
    console.log("→ approval allow (ask mode, bash)");
    await client.request("session/set_mode", { sessionId, modeId: "ask" }, 30_000);
    client.permissionPolicy = "allow";
    rmSync(allowMarker, { force: true });
    const allowTurn = await client.turnPrompt(
      sessionId,
      `Use the bash tool to run exactly: printf zu-ok > ${allowMarker} — then reply done. Do not use any other tool.`,
    );
    if (allowTurn.permissions.length === 0) {
      const text = allowTurn.updates
        .filter((update) => update.sessionUpdate === "agent_message_chunk")
        .map((update) => update.content?.text ?? "")
        .join("");
      const tools = allowTurn.updates.filter((update) => update.sessionUpdate?.startsWith("tool_call"));
      throw new Failure(
        `no permission request was asked; tools=${JSON.stringify(tools.map((u) => u.title ?? u.toolCallId))} text=${text.slice(0, 200)}`,
      );
    }
    assert(allowTurn.stopReason === "end_turn", `allow turn stopReason ${allowTurn.stopReason}`);
    console.log(`  asked: ${allowTurn.permissions[0]?.toolCall?.title ?? "unknown"}`);
    console.log(`  command ran: ${existsSync(allowMarker) ? "yes" : "no"}`);
    if (!existsSync(allowMarker)) throw new Failure("an allowed bash command did not run");

    console.log("→ approval reject (ask mode, bash)");
    client.permissionPolicy = "reject";
    rmSync(rejectMarker, { force: true });
    const rejectTurn = await client.turnPrompt(
      sessionId,
      `Use the bash tool to run exactly: printf zu-bad > ${rejectMarker} — then reply done. Do not use any other tool.`,
    );
    assert(rejectTurn.permissions.length > 0, "no permission request was asked for the reject turn");
    assert(rejectTurn.stopReason === "end_turn", `reject turn stopReason ${rejectTurn.stopReason}`);
    console.log(`  asked: ${rejectTurn.permissions[0]?.toolCall?.title ?? "unknown"}`);
    console.log(`  command ran: ${existsSync(rejectMarker) ? "yes" : "no"}`);
    if (existsSync(rejectMarker)) throw new Failure("a rejected bash command ran anyway");

    console.log("→ /compact (real context reduction)");
    // Several large messages, not one: compaction keeps the most recent
    // ~20k tokens and can only cut at message boundaries.
    let filled: Turn | undefined;
    for (let part = 1; part <= 4; part += 1) {
      const filler = "The quick brown fox jumps over the lazy dog. ".repeat(3500);
      filled = await client.turnPrompt(
        sessionId,
        `Reference material part ${part}. Reply with exactly: noted.\n\n${filler}`,
        240_000,
      );
      assert(filled.stopReason === "end_turn", `filler turn ${part} stopReason ${filled.stopReason}`);
    }
    assert(filled, "no filler turn ran");
    const beforeCompact = lastUsage(filled);
    assert(beforeCompact, "no usage_update before compaction");
    const compacted = await client.turnPrompt(sessionId, "/compact", 240_000);
    assert(compacted.stopReason === "end_turn", `compact turn stopReason ${compacted.stopReason}`);
    const compaction = compacted.updates.find(
      (update) => update.sessionUpdate === "compaction_update" && update.status === "completed",
    );
    assert(compaction, "no completed compaction_update");
    // The gauge is authoritative on the next turn: during compaction the
    // session is mid-rewrite and may not report usage yet.
    const afterCompactTurn = await client.turnPrompt(sessionId, "Reply with exactly: compacted-ok", 120_000);
    assert(afterCompactTurn.stopReason === "end_turn", `post-compact turn stopReason ${afterCompactTurn.stopReason}`);
    const afterCompact = lastUsage(afterCompactTurn) ?? lastUsage(compacted);
    assert(afterCompact, "no usage_update after compaction");
    console.log(`  context: ${beforeCompact.used} -> ${afterCompact.used} of ${afterCompact.size}`);
    if (!(afterCompact.used < beforeCompact.used)) throw new Failure("compaction did not reduce context");

    console.log("→ extension UI confirm (real answer)");
    const extensionPath = join(markerDir, "ask-ui.js");
    writeFileSync(
      extensionPath,
      `export default function (pi) {
  pi.registerCommand("ask-ui", {
    description: "Ask the client to confirm",
    handler: async (_args, ctx) => {
      const ok = await ctx.ui.confirm("Live acceptance", "Allow the extension to continue?");
      ctx.ui.notify(ok ? "confirmed-by-client" : "declined-by-client", "info");
    },
  });
  pi.registerCommand("ask-fields", {
    description: "Ask the client for a choice and an input",
    handler: async (_args, ctx) => {
      const choice = await ctx.ui.select("Pick a value", ["alpha", "beta"]);
      const input = await ctx.ui.input("Type a value", "placeholder");
      ctx.ui.notify(\`choice=\${choice} input=\${input}\`, "info");
    },
  });
}
`,
    );
    const ext = new LiveClient({ env: { ACP_EXTRA_EXTENSION_PATHS: extensionPath } });
    try {
      await ext.request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
      const extCreated = await ext.request<any>("session/new", { cwd: CWD, mcpServers: [] }, 90_000);
      const extSession = String(extCreated.sessionId);
      ext.elicitationPolicy = "accept";
      ext.elicitationContent = { confirmed: true };
      const confirmed = await ext.turnPrompt(extSession, "/ask-ui");
      assert(confirmed.elicitations.length === 1, "extension confirm produced no elicitation");
      assert(
        confirmed.updates.some(
          (update) => update.sessionUpdate === "notice" && String(update.notice?.title ?? "").includes("confirmed-by-client"),
        ),
        "the extension did not receive the accepted answer",
      );
      ext.elicitationPolicy = "decline";
      const declined = await ext.turnPrompt(extSession, "/ask-ui");
      assert(declined.elicitations.length === 1, "extension decline produced no elicitation");
      assert(
        declined.updates.some(
          (update) => update.sessionUpdate === "notice" && String(update.notice?.title ?? "").includes("declined-by-client"),
        ),
        "the extension did not receive the declined answer",
      );
      ext.elicitationPolicy = "accept";
      const fields = await ext.turnPrompt(extSession, "/ask-fields");
      assert(fields.elicitations.length === 2, `select/input produced ${fields.elicitations.length} elicitations`);
      assert(
        fields.updates.some(
          (update) => update.sessionUpdate === "notice" && String(update.notice?.title ?? "").includes("choice=beta input=live-input"),
        ),
        "the extension did not receive the select/input answers",
      );
      console.log("  ok: confirm, select and input answers reached the extension");
    } finally {
      await ext.close();
    }

    console.log("→ subagent dispatch → settle → transcript → abort");
    const dispatched = await client.request<any>(
      "_zu/subagents/dispatch",
      {
        sessionId,
        agent: "scout",
        task: `Use the read/ls tools to list the top-level directories under ${CWD} and reply with a one-line verdict.`,
      },
      60_000,
    );
    const runId = String(dispatched?.run?.runId ?? "");
    assert(runId, "dispatch returned no runId");
    let settledRun: any;
    for (let waited = 0; waited < 180_000; waited += 2000) {
      const listed = await client.request<any>("_zu/subagents/list", { sessionId }, 30_000);
      settledRun = (listed?.runs ?? []).find((run: any) => run.runId === runId);
      if (settledRun && settledRun.status !== "running") break;
      await sleep(2000);
    }
    assert(settledRun, "the subagent run disappeared");
    assert(settledRun.status === "settled", `subagent status ${settledRun.status}`);
    assert(String(settledRun.output ?? "").length > 0, "the subagent produced no output");
    const transcript = await client.request<any>("_zu/subagents/transcript", { sessionId, runId }, 30_000);
    assert((transcript?.messages ?? []).length > 0, "the subagent transcript is empty");
    console.log(`  ok: ${runId} settled (${transcript.messages.length} transcript rows)`);

    const abortRun = await client.request<any>(
      "_zu/subagents/dispatch",
      {
        sessionId,
        agent: "worker",
        task: "Read every file under app/ and write a 3000-word architecture summary. Do not stop early.",
      },
      60_000,
    );
    const abortId = String(abortRun?.run?.runId ?? "");
    assert(abortId, "abort fixture dispatch returned no runId");
    await sleep(1500);
    const aborted = await client.request<any>("_zu/subagents/abort", { sessionId, runId: abortId }, 30_000);
    assert(aborted, "abort returned no result");
    console.log(`  ok: abort accepted for ${abortId}`);

    console.log("→ settled subagent after a fresh process");
    const firstPid = client.pid;
    await client.close();
    await sleep(300);
    assert(!isAlive(firstPid), `pi-acp ${firstPid} is still alive after close`);
    client = new LiveClient();
    await client.request("initialize", { protocolVersion: 1, clientCapabilities: {} }, 60_000);
    await client.request("session/resume", { sessionId, cwd: CWD, mcpServers: [] }, 90_000);
    const roster = await client.request<any>("_zu/subagents/list", { sessionId }, 30_000);
    const persisted = (roster?.runs ?? []).find((run: any) => run.runId === runId);
    assert(persisted, "the settled run is gone after a restart");
    assert(persisted.status === "settled", `restarted run status ${persisted.status}`);
    const persistedTranscript = await client.request<any>(
      "_zu/subagents/transcript",
      { sessionId, runId },
      30_000,
    );
    assert((persistedTranscript?.messages ?? []).length > 0, "the settled transcript is empty after a restart");
    console.log(`  ok: ${runId} and its ${persistedTranscript.messages.length} rows survived the restart`);

    console.log("→ process teardown");
    const pid = client.pid;
    await client.close();
    await sleep(300);
    assert(!isAlive(pid), `pi-acp ${pid} is still alive after close`);
    console.log("  ok: the process is gone after session/close + stdin EOF");

    console.log("LIVE ACCEPTANCE PASSED");
  } finally {
    rmSync(markerDir, { recursive: true, force: true });
    await client.close();
    void homedir;
  }
}

main().catch((error) => {
  console.error(`LIVE ACCEPTANCE FAILED: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
});
