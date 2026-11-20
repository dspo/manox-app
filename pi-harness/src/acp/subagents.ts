/**
 * The dispatch surface: subagent runs created inside the engine, visible to
 * the shell, and owned by their captain.
 *
 * pi's documented subagent extension spawns a `pi` process per task. Zu keeps
 * the run in-process instead: a second AgentSession with an isolated context,
 * a custom session directory, a `parentSession` header and the captain's
 * model. The dispatch tool is registered only on captain sessions, so a
 * subagent can never dispatch again (the two-level rule); the shell reads the
 * roster and transcripts through the wire.
 *
 * Runs are paced per captain (a FIFO queue behind four slots), capped per run
 * (`max_turns` in an agent's frontmatter), and delivered once: a settled run
 * is pushed into the captain's next deliverable point unless the model already
 * read it through `list_agents` or `dispatch(wait:true)`.
 */

import { mkdirSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { Type } from "typebox";
import {
  createAgentSessionFromServices,
  getAgentDir,
  parseFrontmatter,
  SessionManager,
} from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createPiServices } from "../pi/services";
import { tappedBashTool } from "./title-tap";
import { zuStateDir } from "../settings";
/** The subagent surface's own vocabulary (the app has no shared wire type
 * for these; they travel over the `_zu/subagents/*` extension methods). */
export interface PlainMessage {
  role: string;
  text: string;
  at?: number;
}

export type SubagentStatus = "queued" | "running" | "settled" | "failed" | "aborted" | "interrupted";

export interface SubagentRun {
  runId: string;
  captainId: string;
  containerId: string;
  agent: string;
  task: string;
  status: SubagentStatus;
  startedAt: number;
  settledAt?: number;
  verdict?: string;
  output?: string;
  sessionPath?: string;
  /** The result has been delivered to, or read by, the captain's model. */
  collected?: boolean;
}

export interface SubagentTranscriptData {
  run?: SubagentRun;
  messages: PlainMessage[];
}

export interface CaptainLike {
  id: string;
  path: string;
  cwd: string;
  containerId?: string;
  manager: SessionManager;
  session: any;
}

export interface SubagentHost {
  agentDir: string;
  log(message: string): void;
  /** A captain's open record, or undefined when the session is not open. */
  captainFor(id: string): CaptainLike | undefined;
  /** Settled runs persisted in the registry. */
  settledRuns(): SubagentRun[];
  saveSettled(run: SubagentRun): void;
  appendCaptainNote(captainId: string, kind: "dispatch" | "delivery", text: string, data: unknown): void;
  emit(type: string, data?: unknown, sessionId?: string): void;
  projectTrusted(cwd: string): boolean;
  isArchived(captainId: string): boolean;
}

export interface AgentDefinition {
  name: string;
  description: string;
  systemPrompt: string;
  tools?: string[];
  model?: string;
  thinking?: string;
  /** `max_turns`: stop a run that keeps calling tools past this many turns. */
  maxTurns?: number;
  source: "builtin" | "personal" | "project";
}

/** What the runner must do after one child turn (`maxTurns` pacing). */
export type SubagentTurnAction = "continue" | "wrap-up" | "abort";

/** The one seam around the real SDK session; tests drive a fake runner. */
export interface SubagentRunnerInput {
  run: SubagentRun;
  captain: CaptainLike;
  definition: AgentDefinition;
  task: string;
  resume: boolean;
  /** Called on every child `turn_end`; the manager answers with the cap action. */
  onTurn(info?: { continuing?: boolean }): SubagentTurnAction;
  /** The created child session, so the manager can abort/inspect/transcript it. */
  attach(session: any): void;
}

export interface SubagentRunnerOutput {
  output: string;
  aborted: boolean;
  sessionPath: string;
}

export interface SubagentRunner {
  run(input: SubagentRunnerInput): Promise<SubagentRunnerOutput>;
}

/** One captain's in-flight runs; a settled run leaves this map. */
interface LiveRun {
  run: SubagentRun;
  captain: CaptainLike;
  definition: AgentDefinition;
  task: string;
  resume: boolean;
  session: any;
  aborted: boolean;
  /** Has occupied a concurrency slot (queued runs have not). */
  started: boolean;
  abortReason?: "user" | "archive" | "dispose";
  done: Promise<void>;
  release: () => void;
}

interface RunTurnState {
  turns: number;
  wrapped: boolean;
}

/** Settled results reach the captain's model through this binding. */
export interface DeliveryBinding {
  /** true when pi accepted the message synchronously. */
  send(batch: SubagentRun[], options: { triggerTurn: boolean }): boolean;
  isIdle(): boolean;
}

/** A queue behind four slots: the fifth `dispatch` in one response waits. */
export const MAX_CONCURRENT_PER_CAPTAIN = 4;
/** At most this many results share one custom message. */
export const MAX_DELIVERY_BATCH = 6;
/** A run's output is capped before it enters the captain's context. */
export const MAX_DELIVERY_OUTPUT = 50_000;
/** The one steer a capped run receives. */
export const WRAP_UP = "Stop calling tools now and reply with your final result.";

const BUILTIN_AGENTS: AgentDefinition[] = [
  {
    name: "worker",
    description: "General-purpose subagent with the full coding tool set and an isolated context.",
    systemPrompt:
      "You are a worker subagent with full capabilities. Work autonomously on the delegated task, using the tools you need. Finish with a short report: what changed, which files, and anything the captain must know.",
    source: "builtin",
  },
  {
    name: "scout",
    description: "Fast read-only recon: locate code, trace flows, and return compressed context.",
    systemPrompt:
      "You are a scout subagent. Read only: search, inspect and summarize. Do not modify files. Return the paths, symbols and facts the captain needs, compressed.",
    tools: ["read", "grep", "find", "ls"],
    source: "builtin",
  },
  {
    name: "planner",
    description: "Read-only planning: turn a goal into an ordered implementation plan.",
    systemPrompt:
      "You are a planner subagent. Inspect the code read-only and return a concrete, ordered plan with file paths, risks and verification steps. Do not edit.",
    tools: ["read", "grep", "find", "ls"],
    source: "builtin",
  },
  {
    name: "reviewer",
    description: "Read-only review of a change or design; returns findings and a verdict.",
    systemPrompt:
      "You are a reviewer subagent. Inspect the given change read-only and report correctness, edge cases and missing tests. End with a one-line verdict: pass or concerns.",
    tools: ["read", "grep", "find", "ls"],
    source: "builtin",
  },
];

function readAgents(dir: string, source: AgentDefinition["source"]): AgentDefinition[] {
  let names: string[] = [];
  try {
    names = readdirSync(dir).filter((name) => name.endsWith(".md"));
  } catch {
    return [];
  }
  const agents: AgentDefinition[] = [];
  for (const name of names) {
    try {
      const content = readFileSync(join(dir, name), "utf8");
      const { frontmatter, body } = parseFrontmatter<Record<string, unknown>>(content);
      const defined = typeof frontmatter.name === "string" && frontmatter.name.trim() ? frontmatter.name.trim() : name.replace(/\.md$/, "");
      const tools = Array.isArray(frontmatter.tools)
        ? (frontmatter.tools as unknown[]).map(String).filter(Boolean)
        : undefined;
      const rawMaxTurns = frontmatter.max_turns;
      const maxTurns =
        typeof rawMaxTurns === "number" && Number.isInteger(rawMaxTurns) && rawMaxTurns > 0
          ? rawMaxTurns
          : undefined;
      agents.push({
        name: defined,
        description: String(frontmatter.description ?? ""),
        systemPrompt: body.trim(),
        tools,
        model: typeof frontmatter.model === "string" ? frontmatter.model : undefined,
        thinking: typeof frontmatter.thinking === "string" ? frontmatter.thinking : undefined,
        ...(maxTurns === undefined ? {} : { maxTurns }),
        source,
      });
    } catch {
      // A malformed definition is skipped; the builtin of the same name stays.
    }
  }
  return agents;
}

/**
 * Definition precedence is first-come-first-served: builtin, then personal,
 * then the repository's own definitions (trusted projects only). A repository
 * can add agents, never replace `worker`/`scout`/`planner`/`reviewer` — a
 * checked-in `.pi/agents/worker.md` must not rewrite a builtin's tools, model
 * or prompt. Duplicates are logged with the source that lost.
 */
export function discoverAgents(
  cwd: string,
  agentDir: string,
  projectTrusted: boolean,
  log?: (message: string) => void,
): AgentDefinition[] {
  const found = new Map<string, AgentDefinition>();
  const add = (agent: AgentDefinition) => {
    const existing = found.get(agent.name);
    if (existing) {
      log?.(`agent definition ${agent.name} from ${agent.source} ignored: ${existing.source} wins`);
      return;
    }
    found.set(agent.name, agent);
  };
  for (const agent of BUILTIN_AGENTS) add(agent);
  for (const agent of readAgents(join(agentDir, "agents"), "personal")) add(agent);
  if (projectTrusted) {
    for (const agent of readAgents(join(cwd, ".pi", "agents"), "project")) add(agent);
    for (const agent of readAgents(join(cwd, ".agents", "agents"), "project")) add(agent);
  }
  return [...found.values()];
}

/**
 * A run's transcript lives under `zu/subagents/{captainId}/` as
 * `{timestamp}_{runId}.jsonl`. The settled index is the first source; when it
 * lost the path (or the registry entry), the session id suffix still finds it.
 */
export function deriveSubagentSessionFile(
  captainId: string,
  runId: string,
  options: { sessionPath?: string; root?: string } = {},
): string | undefined {
  if (options.sessionPath) return options.sessionPath;
  const root = options.root ?? join(zuStateDir(getAgentDir()), "subagents", captainId);
  let names: string[] = [];
  try {
    names = readdirSync(root);
  } catch {
    return undefined;
  }
  const suffix = `_${runId}.jsonl`;
  const match = names.find((name) => name.endsWith(suffix));
  return match ? join(root, match) : undefined;
}

function plain(message: any): PlainMessage {
  const text =
    typeof message.content === "string"
      ? message.content
      : Array.isArray(message.content)
        ? message.content
            .filter((block: any) => block?.type === "text")
            .map((block: any) => block.text)
            .join("")
        : "";
  return { role: message.role, text, at: message.timestamp };
}

function finalOutput(messages: any[]): string {
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index];
    if (message.role !== "assistant") continue;
    const text = plain(message).text;
    if (text) return text;
  }
  return "";
}

/** The message the captain's model reads when a batch of runs settles. */
export function formatCompletionBatch(batch: SubagentRun[]): string {
  const sections = batch.map((run) => {
    const raw = run.output ?? "";
    const output = raw.length > MAX_DELIVERY_OUTPUT ? `${raw.slice(0, MAX_DELIVERY_OUTPUT)}\n[output truncated at ${MAX_DELIVERY_OUTPUT} characters]` : raw;
    return `## ${run.runId} · ${run.agent} · ${run.status}\nTask: ${run.task}\nOutput:\n${output || "(no output)"}`;
  });
  return `Subagent results:\n\n${sections.join("\n\n")}`;
}

/** SDK seams for the real runner; production leaves both unset. */
export interface SubagentRunnerDeps {
  createServices?: (options: any) => Promise<{ services: any }>;
  createSession?: (options: any) => Promise<{ session: any }>;
}

/**
 * The real runner: one in-process `createAgentSession` per run, with the
 * captain's model and trust, a `parentSession` header, and dispatch hidden
 * from the child's tool layer.
 */
export function createPiSubagentRunner(
  host: Pick<SubagentHost, "agentDir" | "projectTrusted">,
  deps: SubagentRunnerDeps = {},
): SubagentRunner {
  const makeServices = (deps.createServices ?? createPiServices) as typeof createPiServices;
  const makeSession = (deps.createSession ?? createAgentSessionFromServices) as typeof createAgentSessionFromServices;
  return {
    async run(input: SubagentRunnerInput): Promise<SubagentRunnerOutput> {
      const { run, captain, definition, task, resume } = input;
      const dir = join(zuStateDir(host.agentDir), "subagents", captain.id);
      mkdirSync(dir, { recursive: true });
      const parent = captain.manager.getSessionFile() ?? captain.path;
      const manager = resume && run.sessionPath
        ? SessionManager.open(run.sessionPath)
        : SessionManager.create(captain.cwd, dir, {
            id: run.runId,
            parentSession: parent || undefined,
          });
      const built = await makeServices({
        cwd: captain.cwd,
        agentDir: host.agentDir,
        trusted: host.projectTrusted(captain.cwd),
        noExtensions: true,
        appendSystemPrompt: [definition.systemPrompt],
      });
      let model = captain.session.model;
      if (definition.model?.includes("/")) {
        const [provider, ...rest] = definition.model.split("/");
        model = captain.session.modelRuntime?.getModel(provider, rest.join("/")) ?? model;
      }
      const created = await makeSession({
        services: built.services,
        sessionManager: manager,
        model,
        thinkingLevel: (definition.thinking as any) ?? captain.session.thinkingLevel,
        tools: definition.tools,
        // Depth defence: even if a child ever loads extensions, it cannot
        // dispatch or list (the two-level rule at the tool layer).
        excludeTools: ["dispatch", "list_agents"],
        // Same title tap as the captain's: a subagent's bash writes the
        // session's channel too (issue #73).
        customTools: [
          tappedBashTool(captain.cwd, built.services.settingsManager),
        ],
      });
      const session = created.session;
      input.attach(session);
      const unsubscribe = definition.maxTurns
        ? session.subscribe((event: any) => {
            if (event?.type !== "turn_end") return;
            const content = (event.message as any)?.content;
            const continuing = Array.isArray(content) && content.some((block: any) => block?.type === "toolCall");
            const action = input.onTurn({ continuing });
            if (action === "wrap-up") {
              void session.sendUserMessage(WRAP_UP, { deliverAs: "steer" }).catch(() => {});
            } else if (action === "abort") {
              void session.abort().catch(() => {});
            }
          })
        : () => {};
      let failure: unknown;
      try {
        await session.prompt(task);
      } catch (error) {
        failure = error;
      } finally {
        unsubscribe();
      }
      const aborted = session.state?.messages?.some((message: any) => message?.stopReason === "aborted") ?? false;
      const output = finalOutput(session.state?.messages ?? []);
      const sessionPath = manager.getSessionFile() ?? "";
      session.dispose();
      // An abort rejects the prompt in some pi versions; the session state is
      // still the truth. A non-abort failure is the run's failure.
      if (failure !== undefined && !aborted) throw failure;
      return { output, aborted, sessionPath };
    },
  };
}

/**
 * Owns live runs and projects the settled ones from the registry. Live runs
 * are intentionally memory-only (a restart cannot resume them); settled runs
 * keep their session file under `zu/subagents/{captainId}/`.
 */
export class SubagentManager {
  private live = new Map<string, LiveRun>();
  /** Per captain: run ids waiting for a slot, in dispatch order. */
  private queues = new Map<string, string[]>();
  private turns = new Map<string, RunTurnState>();
  /** Settled runs not yet delivered to (or read by) the captain's model. */
  private pending: SubagentRun[] = [];
  private delivery: DeliveryBinding | null = null;
  private disposed = false;
  private seq = 0;
  private readonly runner: SubagentRunner;

  constructor(private host: SubagentHost, runner?: SubagentRunner) {
    this.runner = runner ?? createPiSubagentRunner(host);
  }

  private nextRunId(): string {
    return `run-${Date.now().toString(36)}-${++this.seq}`;
  }

  private definitionFor(captain: CaptainLike, name: string): AgentDefinition | null {
    return (
      discoverAgents(captain.cwd, this.host.agentDir, this.host.projectTrusted(captain.cwd), (line) =>
        this.host.log(line),
      ).find((agent) => agent.name === name) ?? null
    );
  }

  list(filter: { containerId?: string; captainId?: string }): SubagentRun[] {
    const live = [...this.live.values()].map((entry) => entry.run);
    const settled = this.host.settledRuns();
    const byId = new Map<string, SubagentRun>();
    for (const run of settled) byId.set(run.runId, run);
    // A live run is newer than its settled copy, if a resume kept the run id.
    for (const run of live) byId.set(run.runId, run);
    return [...byId.values()]
      .filter((run) => !filter.captainId || run.captainId === filter.captainId)
      .filter((run) => !filter.containerId || run.containerId === filter.containerId)
      .sort((a, b) => b.startedAt - a.startedAt);
  }

  run(runId: string): SubagentRun | undefined {
    return this.live.get(runId)?.run ?? this.host.settledRuns().find((run) => run.runId === runId);
  }

  /**
   * Reading a terminal run claims its result: the model has seen it, so the
   * completion push must not show it a second time.
   */
  claim(runIds: string[]): void {
    for (const runId of runIds) {
      const run = this.run(runId);
      if (!run || run.collected || run.status === "running" || run.status === "queued") continue;
      run.collected = true;
      this.pending = this.pending.filter((entry) => entry.runId !== runId);
      if (!this.live.has(runId)) this.host.saveSettled(run);
    }
  }

  setDelivery(binding: DeliveryBinding | null): void {
    this.delivery = binding;
  }

  /**
   * Push settled results at the captain's next deliverable point. One custom
   * message per flush (at most six runs); a rejected send restores the runs
   * to `pending` so nothing is lost. `triggerTurn` is true only when the
   * captain is idle — a wake-up, not a queue entry.
   */
  flushCompletions(options: { triggerTurn?: boolean } = {}): void {
    const binding = this.delivery;
    if (!binding || this.pending.length === 0) return;
    const batch = this.pending.splice(0, MAX_DELIVERY_BATCH);
    for (const run of batch) {
      run.collected = true;
      if (!this.live.has(run.runId)) this.host.saveSettled(run);
    }
    try {
      const accepted = binding.send(batch, { triggerTurn: options.triggerTurn ?? false });
      if (!accepted) throw new Error("delivery was not accepted");
    } catch (error) {
      this.host.log(`subagent delivery failed: ${String(error)}`);
      for (const run of batch) {
        run.collected = false;
        if (!this.live.has(run.runId)) this.host.saveSettled(run);
      }
      this.pending.unshift(...batch);
    }
  }

  /** A user stop must not wake the captain for results it did not ask for. */
  markNonWaking(): void {
    this.flushCompletions({ triggerTurn: false });
  }

  /** Starts a new run and returns its ack before the model turn completes. */
  spawn(captainId: string, agentName: string, task: string): SubagentRun {
    const captain = this.host.captainFor(captainId);
    if (!captain) throw new Error("captain session is not open");
    if (this.host.isArchived(captainId)) throw new Error("the captain's container is archived");
    const definition = this.definitionFor(captain, agentName);
    if (!definition) throw new Error(`unknown agent: ${agentName}`);
    const run: SubagentRun = {
      runId: this.nextRunId(),
      captainId,
      containerId: captain.containerId ?? "",
      agent: definition.name,
      task,
      status: "queued",
      startedAt: Date.now(),
      collected: false,
    };
    this.begin(captain, definition, run, task, false);
    return run;
  }

  /** Re-runs a settled subagent by opening its session file again. */
  resume(captainId: string, runId: string, task: string): SubagentRun {
    const captain = this.host.captainFor(captainId);
    if (!captain) throw new Error("captain session is not open");
    if (this.live.has(runId)) throw new Error("run is still live");
    const previous = this.host.settledRuns().find((run) => run.runId === runId);
    if (!previous?.sessionPath) throw new Error("settled run has no session file");
    const definition = this.definitionFor(captain, previous.agent);
    if (!definition) throw new Error(`unknown agent: ${previous.agent}`);
    const run: SubagentRun = {
      ...previous,
      task,
      status: "queued",
      startedAt: Date.now(),
      settledAt: undefined,
      verdict: undefined,
      output: undefined,
      collected: false,
    };
    this.begin(captain, definition, run, task, true);
    return run;
  }

  private begin(captain: CaptainLike, definition: AgentDefinition, run: SubagentRun, task: string, resume: boolean): void {
    this.host.appendCaptainNote(
      captain.id,
      "dispatch",
      `${resume ? "Resumed" : "Dispatched"} ${run.agent} · ${run.runId}`,
      { runId: run.runId, agent: run.agent, task },
    );
    let release = () => {};
    const done = new Promise<void>((resolve) => {
      release = resolve;
    });
    const entry: LiveRun = {
      run,
      captain,
      definition,
      task,
      resume,
      session: null,
      aborted: false,
      started: false,
      done,
      release,
    };
    this.live.set(run.runId, entry);
    const queue = this.queues.get(captain.id) ?? [];
    queue.push(run.runId);
    this.queues.set(captain.id, queue);
    this.pump(captain.id);
  }

  /** Starts queued runs in FIFO order while the captain has a free slot. */
  private pump(captainId: string): void {
    if (this.disposed) return;
    const queue = this.queues.get(captainId);
    if (!queue) return;
    let slots =
      MAX_CONCURRENT_PER_CAPTAIN -
      [...this.live.values()].filter(
        (entry) => entry.run.captainId === captainId && entry.started && !entry.run.settledAt,
      ).length;
    while (slots > 0 && queue.length > 0) {
      const runId = queue.shift()!;
      const entry = this.live.get(runId);
      if (!entry || entry.started || entry.run.status !== "queued") continue;
      entry.started = true;
      entry.run.status = "running";
      this.host.emit("subagent.started", { run: entry.run }, runId);
      slots -= 1;
      void this.execute(entry);
    }
  }

  private async execute(entry: LiveRun): Promise<void> {
    const { run, captain, definition, task, resume } = entry;
    try {
      const result = await this.runner.run({
        run,
        captain,
        definition,
        task,
        resume,
        onTurn: (info) => this.onTurn(entry, info),
        attach: (session) => {
          entry.session = session;
          // An abort that raced session creation must land as soon as there is
          // something to abort.
          if (entry.aborted) void session.abort().catch(() => {});
        },
      });
      run.sessionPath = result.sessionPath || run.sessionPath;
      const status: SubagentStatus =
        run.status === "interrupted" ? "interrupted" : entry.aborted || result.aborted ? "aborted" : "settled";
      this.settle(
        run,
        status,
        result.output,
        status === "settled" ? (result.output.match(/verdict:\s*(.+)/i)?.[1] ?? "completed") : status,
      );
    } catch (error: any) {
      this.settle(run, "failed", String(error?.message ?? error));
    }
  }

  /**
   * Turn cap: steer once at the cap when the child would keep going, abort on
   * the next turn. A child that finishes within the cap keeps its result.
   */
  private onTurn(entry: LiveRun, info?: { continuing?: boolean }): SubagentTurnAction {
    const cap = entry.definition.maxTurns;
    if (!cap) return "continue";
    const state = this.turns.get(entry.run.runId) ?? { turns: 0, wrapped: false };
    state.turns += 1;
    this.turns.set(entry.run.runId, state);
    if (state.turns > cap) {
      entry.aborted = true;
      return "abort";
    }
    if (state.turns >= cap && info?.continuing && !state.wrapped) {
      state.wrapped = true;
      return "wrap-up";
    }
    return "continue";
  }

  private settle(run: SubagentRun, status: SubagentStatus, output: string, verdict: string = status): void {
    const entry = this.live.get(run.runId);
    if (!entry) return;
    this.live.delete(run.runId);
    this.turns.delete(run.runId);
    run.status = status;
    run.output = output;
    run.verdict = verdict;
    run.settledAt = Date.now();
    this.host.saveSettled(run);
    this.host.appendCaptainNote(
      run.captainId,
      "delivery",
      `${run.agent} ${status} · ${run.runId}`,
      { runId: run.runId, status, verdict, summary: output.slice(0, 2000) },
    );
    this.host.emit("subagent.settled", { run }, run.runId);
    // A user-stopped run is appended to the record, but must not wake the
    // captain; an archive or shutdown has no captain left to deliver to.
    const deliverable =
      this.delivery &&
      !run.collected &&
      entry.abortReason !== "user" &&
      entry.abortReason !== "archive" &&
      entry.abortReason !== "dispose";
    if (deliverable) {
      this.pending.push(run);
      if (this.delivery!.isIdle()) queueMicrotask(() => this.flushCompletions({ triggerTurn: true }));
    }
    entry.release();
    this.pump(run.captainId);
  }

  async wait(runId: string): Promise<SubagentRun | undefined> {
    const entry = this.live.get(runId);
    if (entry) await entry.done;
    return this.run(runId);
  }

  async inject(runId: string, message: string): Promise<void> {
    const entry = this.live.get(runId);
    if (!entry?.session) throw new Error("run is not live");
    await entry.session.sendUserMessage(message, { deliverAs: "steer" });
  }

  async abort(runId: string): Promise<void> {
    const entry = this.live.get(runId);
    if (!entry) return;
    entry.abortReason = "user";
    entry.aborted = true;
    if (!entry.started) {
      // A queued run never created a session; cancelling it is just settling.
      this.settle(entry.run, "aborted", "cancelled before start", "aborted");
      return;
    }
    await entry.session?.abort();
  }

  /** Cascade: a captain's archive interrupts every live run of it. */
  async archiveCaptain(captainId: string): Promise<void> {
    // Drop the queue first: settling a queued run pumps its captain, and
    // pumping here would start a run we are about to cancel.
    this.queues.delete(captainId);
    const affected = [...this.live.values()].filter((entry) => entry.run.captainId === captainId);
    for (const entry of affected) {
      entry.abortReason = "archive";
      entry.run.status = "interrupted";
      entry.aborted = true;
      if (!entry.started) {
        this.settle(entry.run, "interrupted", entry.run.output ?? "", "interrupted");
        continue;
      }
      try {
        await entry.session?.abort();
      } catch {
        // A run that has not created its session yet still gets settled below.
      }
      if (this.live.has(entry.run.runId)) {
        this.settle(entry.run, "interrupted", entry.run.output ?? "", "interrupted");
      }
    }
  }

  /**
   * The run's transcript: live from the child session, settled from the
   * index path, then from the `_{runId}.jsonl` suffix. A miss is an explicit
   * error — an empty transcript would look like a successful read.
   */
  transcript(runId: string): SubagentTranscriptData {
    const run = this.run(runId);
    if (!run) throw new Error("run not found");
    const entry = this.live.get(runId);
    if (entry?.session) {
      return { run, messages: (entry.session.state?.messages ?? []).map(plain) };
    }
    // A queued run, or one whose child session is still being built, has no
    // transcript yet; that is an empty read, not a lost file.
    if (entry) return { run, messages: [] };
    const root = join(zuStateDir(this.host.agentDir), "subagents", run.captainId);
    const path = deriveSubagentSessionFile(run.captainId, runId, { sessionPath: run.sessionPath, root });
    if (!path) {
      throw new Error(`run ${runId}: its transcript is not recoverable after a restart`);
    }
    try {
      const manager = SessionManager.open(path);
      return { run, messages: (manager.buildSessionContext().messages ?? []).map(plain) };
    } catch (error: any) {
      throw new Error(`run ${runId}: its transcript cannot be read (${String(error?.message ?? error)})`);
    }
  }

  dispose(): void {
    this.disposed = true;
    for (const entry of this.live.values()) {
      entry.abortReason = "dispose";
      entry.run.status = "interrupted";
      entry.aborted = true;
      try {
        entry.session?.dispose();
      } catch {
        // Already gone.
      }
      entry.release();
    }
    this.live.clear();
    this.queues.clear();
    this.turns.clear();
    this.pending = [];
    this.delivery = null;
  }
}

function textResult(text: string, details: unknown = {}, isError = false) {
  return { content: [{ type: "text" as const, text }], details, ...(isError ? { isError: true } : {}) };
}

/** The dispatch tools, registered only on a captain's session. */
export function dispatchExtension(manager: SubagentManager, captainId: string, isIdle: () => boolean = () => true) {
  return (pi: ExtensionAPI): void => {
    // Completion delivery is a custom message: it participates in the LLM
    // context (unlike `appendCustomEntry`), and one batch is one message.
    pi.on("session_start", () => {
      manager.setDelivery({
        send: (batch, options) => {
          try {
            pi.sendMessage(
              {
                customType: "zu.subagent.completion",
                content: [{ type: "text", text: formatCompletionBatch(batch) }],
                display: true,
                details: { runs: batch.map((run) => run.runId) },
              },
              { triggerTurn: options.triggerTurn },
            );
            return true;
          } catch {
            return false;
          }
        },
        isIdle,
      });
    });
    // At a turn boundary the message joins the next request without waking
    // the captain; `agent_settled` after a user stop must not wake either.
    pi.on("turn_end", (event) => {
      if (event.outcome !== "aborted") manager.flushCompletions({ triggerTurn: false });
    });
    pi.on("agent_settled", (event) => {
      if (event.aborted) manager.markNonWaking();
    });

    pi.registerTool({
      name: "dispatch",
      label: "Dispatch subagent",
      description:
        "Dispatch a subagent run in an isolated context. Without a runId, spawn `agent` with `task`. With a settled runId and a task, resume it. action=inject sends a message to a live run; action=abort stops it. Returns the run id immediately unless wait=true. At most four runs execute per captain; the rest queue. Read a result with wait=true or list_agents — both claim it so it is delivered only once.",
      promptSnippet: "dispatch a subagent run / resume / inject / abort",
      parameters: Type.Object({
        action: Type.Optional(
          Type.Union([Type.Literal("spawn"), Type.Literal("resume"), Type.Literal("inject"), Type.Literal("abort")]),
        ),
        agent: Type.Optional(Type.String()),
        task: Type.Optional(Type.String()),
        runId: Type.Optional(Type.String()),
        message: Type.Optional(Type.String()),
        wait: Type.Optional(Type.Boolean()),
      }),
      async execute(_toolCallId: string, params: any, _signal: any, _onUpdate: any, _ctx: any) {
        try {
          const action = params.action ?? (params.runId ? "resume" : "spawn");
          if (action === "abort") {
            if (!params.runId) return textResult("runId is required to abort", {}, true);
            await manager.abort(params.runId);
            return textResult(`aborted ${params.runId}`, { runId: params.runId });
          }
          if (action === "inject") {
            if (!params.runId || !params.message) return textResult("runId and message are required", {}, true);
            await manager.inject(params.runId, params.message);
            return textResult(`injected into ${params.runId}`, { runId: params.runId });
          }
          if (action === "resume") {
            if (!params.runId || !params.task) return textResult("runId and task are required to resume", {}, true);
            const run = manager.resume(captainId, params.runId, params.task);
            if (params.wait) {
              const settled = await manager.wait(run.runId);
              manager.claim([run.runId]);
              return textResult(settled?.output ?? "(no output)", { run: settled });
            }
            return textResult(`resumed ${run.runId} (${run.agent})`, { run });
          }
          if (!params.agent || !params.task) return textResult("agent and task are required", {}, true);
          const run = manager.spawn(captainId, params.agent, params.task);
          if (params.wait) {
            const settled = await manager.wait(run.runId);
            manager.claim([run.runId]);
            return textResult(settled?.output ?? "(no output)", { run: settled });
          }
          return textResult(`dispatched ${run.agent} as ${run.runId}`, { run });
        } catch (error: any) {
          return textResult(String(error?.message ?? error), {}, true);
        }
      },
    });

    pi.registerTool({
      name: "list_agents",
      label: "List subagents",
      description: "List the captain's subagent runs: live and settled, with status and output. Reading a settled run claims its result.",
      parameters: Type.Object({}),
      async execute(_toolCallId: string, _params: any) {
        const runs = manager.list({ captainId });
        manager.claim(runs.map((run) => run.runId));
        return textResult(
          runs.length === 0
            ? "no subagent runs"
            : runs
                .map(
                  (run) =>
                    `${run.runId} ${run.agent} ${run.status}${run.verdict ? ` (${run.verdict})` : ""}: ${run.task.slice(0, 120)}${run.output ? `\n  ${run.output.slice(0, 400).replace(/\n/g, "\n  ")}` : ""}`,
                )
                .join("\n"),
          { runs },
        );
      },
    });
  };
}
