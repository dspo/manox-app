/**
 * The pi glue behind the ACP surface: one in-process pi SDK session per ACP
 * session, its events mapped onto `session/update` payloads (v2 §4.1 — the
 * adapter embeds the SDK, no RPC subprocess).
 *
 * This is the createAgentSession wiring behind a protocol peer: no Zu
 * registry, no wire events, no containers — the ACP sessionId is pi's
 * sessionId and pi's session files stay the only durable authority. The
 * dispatch extension and the subagent manager are restored, scoped to the
 * captain session; the ACP process is persistent for the conversation.
 *
 * Env the adapter reads:
 *   ACP_NO_EXTENSIONS=1            — load pi with no extensions at all
 *   ACP_EXTRA_EXTENSION_PATHS=a,b  — additional extension paths
 *
 * Project trust is conservative: a saved decision in pi's trust store is
 * honored; an unsaved one is treated as untrusted (project-local extensions
 * and settings stay unloaded) and logged.
 */

import { mkdirSync, readFileSync, readdirSync, renameSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import {
  createAgentSessionFromServices,
  getAgentDir,
  SessionManager,
} from "@earendil-works/pi-coding-agent";
import type {
  AgentSession,
  AgentSessionEvent,
  ExtensionAPI,
  ExtensionUIContext,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";
import type {
  AvailableCommand,
  CreateElicitationRequest,
  CreateElicitationResponse,
  RequestPermissionRequest,
  RequestPermissionResponse,
  SessionConfigOption,
  SessionModeState,
  SessionUpdate,
  StopReason,
} from "@agentclientprotocol/sdk";
import { codedError, JSONRPC_CODE } from "./errors";
import {
  assistantDeltaUpdate,
  describeToolCall,
  isRecord,
  piToolKind,
  settledUpdate,
  stopReasonFromPi,
  toolCallEndUpdate,
  toolCallPartialUpdate,
  toolCallStartUpdate,
  toolStats,
  userEntryUpdates,
  type PiMeta,
} from "./mapping";
import { replayUpdates, type ReplayEntry } from "./replay";
import { askExtensionEntry, type AskBridge } from "../extensions/ask";
import { todosExtensionEntry } from "../extensions/todos";
import { visualizeExtensionEntry } from "../extensions/visualize";
import { composeExtensions, ZU_EXTENSIONS } from "../extensions/registry";
import { DISPATCH_RULE, promptSectionExtension } from "../extensions/prompt-section";
import { ARTIFACTS_RULE, artifactsExtension } from "../extensions/artifacts";
import { readZuSettings, zuStateDir } from "../settings";
import { applyTodoOp, readPlan, todosRoot } from "../todos/core";
import { PLAN_NOTE_TYPE, PlanSteer, planEditFacts } from "../todos/steer";
import { TODO_TOOL_NAMES } from "../todos/tools";
import { createPiServices } from "../pi/services";
import { PI_VERSION } from "../version";
import type { AcpMcpServer } from "../pi/mcp";
import type { TrustElicit } from "../pi/trust";
import { SubagentManager, dispatchExtension, type SubagentHost, type SubagentRun } from "./subagents";
import { tappedBashTool } from "./title-tap";
import {
  AnswerRegistry,
  INTERACTIVE_TOOLS,
  TOOL_RESPONSE_TYPE,
  repairDanglingInteractiveCalls,
} from "../interactions";
import type {
  HarnessClientBridge,
  HarnessPorts,
  HarnessSession,
  HarnessSessionContext,
  HarnessLog,
  SessionBanner,
  SessionBannerWarning,
} from "./app";

type PiAgentSession = AgentSession;
type ThinkingLevel = Parameters<PiAgentSession["setThinkingLevel"]>[0];

/** The runtime face `sessionConfigOptions` reads — pi 1.5.0's ModelRuntime,
 * whose `getProviderAuthStatus(providerId)` is the same API the CLI uses for
 * its auth mapping (`configured` + `source` + optional `label`). */
export interface ConfigOptionsRuntime {
  getAvailableSnapshot?(): readonly unknown[];
  getProvider?(providerId: string): { name?: string } | undefined;
  getProviderAuthStatus(providerId: string): {
    configured: boolean;
    source?: string;
    label?: string;
  };
}

/** The `_meta` a model option carries (issue #54): the display name and
 * credential source of its provider, plus the context window when known.
 * `authSource` is pi's own source word (stored / environment / …), `none`
 * when the provider has no credential — the menu says so instead of
 * leaving the user to find out at pick time. */
export function modelMeta(
  runtime: ConfigOptionsRuntime | undefined,
  providerId: string,
  contextWindow?: number,
): { contextWindow?: number; providerName: string; authSource: string; authLabel?: string } | undefined {
  if (!runtime) return undefined;
  const status = runtime.getProviderAuthStatus(providerId);
  const source = status.configured ? (status.source ?? "unknown") : "none";
  return {
    ...(contextWindow !== undefined && Number.isFinite(contextWindow) ? { contextWindow } : {}),
    providerName: runtime.getProvider?.(providerId)?.name ?? providerId,
    authSource: source,
    ...(status?.label ? { authLabel: status.label } : {}),
  };
}

/** The config selectors one session advertises: the model list from the
 * runtime's availability snapshot (each option with its `_meta` facts) and
 * the thought ladder from the session. Exported for tests — the pi glue
 * calls it with its own pieces. */
export function sessionConfigOptions(
  runtime: ConfigOptionsRuntime | undefined,
  enabledPatterns: string[] | null | undefined,
  current: { provider: string; id: string; name?: string; contextWindow?: number } | undefined,
  levels: readonly string[],
  thinkingLevel: string,
): SessionConfigOption[] {
  const all = runtime?.getAvailableSnapshot?.() ?? [];
  const models = enabledPatterns
    ? all.filter((model) => modelAllowed(enabledPatterns, model as { provider: string; id: string; name?: string }))
    : all;
  const options = models.map((raw) => {
    const model = raw as { provider: string; id: string; name?: string; contextWindow?: number };
    return {
      value: `${model.provider}/${model.id}`,
      name: model.name ?? `${model.provider}/${model.id}`,
      description: model.provider,
      // Issue #54: the menu's provider rows answer "why is this provider
      // listed" (credential source) and the model rows show the context
      // window, without a second model-catalog process.
      _meta: modelMeta(runtime, model.provider, model.contextWindow),
    };
  });
  // The active model must stay selectable even when a scope pattern is
  // narrower than the session's restored model.
  if (current && !options.some((option) => option.value === `${current.provider}/${current.id}`)) {
    options.unshift({
      value: `${current.provider}/${current.id}`,
      name: current.name ?? `${current.provider}/${current.id}`,
      description: current.provider,
      _meta: modelMeta(runtime, current.provider),
    });
  }
  const levels2 = levels.map((level) => ({ value: level, name: level }));
  const configs: SessionConfigOption[] = [];
  if (options.length > 0) {
    configs.push({
      id: "model",
      name: "Model",
      category: "model",
      type: "select",
      currentValue: current ? `${current.provider}/${current.id}` : options[0].value,
      options,
    });
  }
  if (levels2.length > 0) {
    configs.push({
      id: "thought_level",
      name: "Thinking",
      category: "thought_level",
      type: "select",
      currentValue: thinkingLevel,
      options: levels2,
    });
  }
  return configs;
}

/** `enabledModels` patterns are fuzzy in pi; this is the conservative subset. */
function modelAllowed(patterns: string[], model: { provider: string; id: string; name?: string }): boolean {
  const ref = `${model.provider}/${model.id}`;
  const name = model.name ?? ref;
  return patterns.some((pattern) => {
    if (pattern === ref) return true;
    if (pattern.endsWith("/*")) return model.provider === pattern.slice(0, -2);
    if (pattern.includes("/")) return ref.startsWith(pattern) || name.toLowerCase().includes(pattern.toLowerCase());
    return model.id.toLowerCase().includes(pattern.toLowerCase()) || name.toLowerCase().includes(pattern.toLowerCase());
  });
}

/** What the ACP layer can carry into one Pi session's construction. */
export interface PiSessionOptions {
  agentDir?: string;
  /** The ACP client's trust question (session/new is still in flight). */
  elicit?: TrustElicit;
  /** Client-supplied MCP servers (captain only). */
  mcpServers?: AcpMcpServer[];
  /** A captain's decision, inherited by a subagent. */
  trusted?: boolean;
  /** The client-minted session id (issue #56), already validated by
   * `assertFreeSessionId`; pi casts the session file with it. */
  sessionId?: string;
}

/** The approval-policy modes advertised through `session/set_mode`. */
const APPROVAL_MODES: SessionModeState = {
  currentModeId: "ask",
  availableModes: [
    { id: "ask", name: "Ask", description: "Ask before destructive tools and commands." },
    { id: "auto", name: "Auto", description: "Run tools without asking." },
    { id: "read-only", name: "Read-only", description: "Block writes, commands and network tools." },
  ],
};

/** Tools that are safe to run without a prompt in `ask` mode. */
const READ_ONLY_TOOLS = new Set([
  "read",
  "ls",
  "find",
  "grep",
  "glob",
  "search",
  "view",
  // A1: asking the user a question only reads their intent — it writes no
  // workspace, runs no command and touches no network, so it must not open
  // the approval card in ask mode nor trip read-only's write block.
  "ask_user_question",
  // A2: the plan tools write the session's own state file (~/.pi/zu/todos/),
  // never the workspace — they pass the gate for the same reason, so a
  // read-only run can still keep its plan honest.
  ...TODO_TOOL_NAMES,
]);

/** The session's live approval gate: mode, remembered allows, client bridge. */
export class PermissionGate {
  mode = "ask";
  sessionId = "";
  bridge: HarnessClientBridge | null = null;
  private readonly allowed = new Set<string>();

  constructor(private readonly log: HarnessLog) {}

  extension(): (pi: ExtensionAPI) => void {
    return (pi: ExtensionAPI) => {
      pi.on("tool_call", (async (event: any) => this.check(event)) as never);
    };
  }

  private async check(event: ToolCallLike): Promise<{ block?: boolean; reason?: string } | void> {
    const tool = String(event.toolName ?? "");
    if (this.mode === "auto") return;
    if (this.mode === "read-only") {
      if (READ_ONLY_TOOLS.has(tool)) return;
      return { block: true, reason: "read-only mode: writes, commands and network tools are blocked" };
    }
    if (READ_ONLY_TOOLS.has(tool)) return;
    if (this.allowed.has(tool)) return;
    if (!this.bridge) {
      this.log(`permission required for ${tool}, but no client bridge is attached`);
      return { block: true, reason: "no approval channel is attached" };
    }
    const title = describeToolCall(tool, event.input);
    try {
      const response = await this.bridge.requestPermission({
        sessionId: this.sessionId,
        toolCall: {
          toolCallId: String(event.toolCallId ?? ""),
          title,
          kind: piToolKind(tool),
          status: "pending",
          ...(isRecord(event.input) ? { rawInput: event.input } : {}),
        },
        options: [
          { optionId: "allow_once", name: "Allow once", kind: "allow_once" },
          { optionId: "allow_always", name: "Always allow", kind: "allow_always" },
          { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
      });
      const outcome = (response as { outcome?: { outcome?: string; optionId?: string } }).outcome;
      if (outcome?.outcome !== "selected") {
        return { block: true, reason: "the approval was cancelled" };
      }
      if (outcome.optionId === "allow_always") this.allowed.add(tool);
      if (outcome.optionId === "allow_once" || outcome.optionId === "allow_always") return;
      return { block: true, reason: "rejected by the user" };
    } catch (error) {
      this.log(`permission request failed: ${String(error)}`);
      return { block: true, reason: "the approval request failed" };
    }
  }
}

interface ToolCallLike {
  toolName?: string;
  toolCallId?: string;
  input?: unknown;
}

/**
 * The captain's extension factories, in binding order:
 * 1. permission gate — must observe every tool call, so it binds first;
 * 2. prompt sections — byte-stable captain rules (see ../extensions/prompt-section.ts);
 *    stateless, so their position only needs to stay between the two stateful ones;
 * 3. artifacts guard — blocks `write` of new files under system temp roots
 *    (see ../extensions/artifacts.ts); a second `tool_call` handler beside the gate;
 * 4. dispatch — registers the `dispatch` tool, whose usage the rules above shape.
 */
export function hostExtensionFactories(
  gate: Pick<PermissionGate, "extension">,
  subagentManager: SubagentManager,
  captainId: string,
  log?: HarnessLog,
  isIdle: () => boolean = () => false,
) {
  return [
    gate.extension(),
    promptSectionExtension("zu-dispatch", DISPATCH_RULE, log),
    promptSectionExtension("zu-artifacts", ARTIFACTS_RULE, log),
    artifactsExtension(log),
    dispatchExtension(subagentManager, captainId, isIdle),
  ];
}

function extraExtensionPaths(): string[] {
  return (process.env.ACP_EXTRA_EXTENSION_PATHS ?? "")
    .split(",")
    .map((path) => path.trim())
    .filter(Boolean);
}

function extensionsDisabled(): boolean {
  return process.env.ACP_NO_EXTENSIONS === "1";
}

class PiHarnessSession implements HarnessSession {
  readonly id: string;
  readonly cwd: string;

  private readonly manager: SessionManager;
  private readonly session: PiAgentSession;
  private readonly log: HarnessLog;
  private push: ((update: SessionUpdate) => void) | undefined;
  /** Chunks of one pi message share an ACP messageId; bumped per message. */
  private messageSeq = 0;
  private currentMessageId = "";
  /** The running assistant message's anchor facts, read at `message_start`. */
  private currentMessageAt: number | undefined;
  private currentModel: PiMeta["model"] | undefined;
  /** The last settled assistant stop reason of the running turn. */
  private lastStop: string | undefined;
  /** The approval gate the permission extension uses. */
  private readonly gate: PermissionGate;
  /** The settings the session's services loaded (model scope, defaults). */
  private readonly settingsManager: SettingsManager;
  /** The subagent manager bound to this captain. */
  private readonly subagentManager: SubagentManager;
  /** The interactive-tool mailbox (B2): late answers and supersede. */
  private readonly answerRegistry: AnswerRegistry;
  /** The plan store's directory (`~/.pi/zu/todos`); the tools get the same
   * derivation from the extension entry, so both write one file. */
  private readonly todosDir: string;
  /** Whether `disabledExtensions` left the `todos` tools registered. */
  private readonly planToolsEnabled: boolean;
  /** The panel-edit steering: notes into the conversation, wakes at settle,
   * and interrupts a running turn before a removal lands. */
  private readonly planSteer: PlanSteer;
  /** Compaction ids are per session and stable across start/end events. */
  private compactionSeq = 0;
  private currentCompactionId = "";
  /** Each call's arguments, kept so the settle can turn an edit into diff
   * content (mapping.ts); the card's rawInput carries them until then. */
  private readonly argsByToolCall = new Map<string, unknown>();

  private constructor(
    manager: SessionManager,
    session: PiAgentSession,
    cwd: string,
    log: HarnessLog,
    gate: PermissionGate,
    settingsManager: SettingsManager,
    subagentManager: SubagentManager,
    answerRegistry: AnswerRegistry,
    todosDir: string,
    planToolsEnabled: boolean,
  ) {
    this.manager = manager;
    this.session = session;
    this.cwd = cwd;
    this.log = log;
    this.gate = gate;
    this.settingsManager = settingsManager;
    this.subagentManager = subagentManager;
    this.answerRegistry = answerRegistry;
    this.todosDir = todosDir;
    this.planToolsEnabled = planToolsEnabled;
    this.planSteer = new PlanSteer({
      // The idle probe reads pi's own session state, whichever shape 1.5.0
      // ships it in (the same defensive read the dispatch factory uses).
      idle: () => {
        const session = this.session as unknown as { isIdle?: unknown } | null;
        return typeof session?.isIdle === "function" ? Boolean(session.isIdle()) : Boolean(session?.isIdle);
      },
      abort: () => this.session.abort(),
      inject: async (text, { wake }) => {
        // The note joins the context (not appendCustomEntry) and, when the
        // session is between turns, wakes one so the plan is re-read now.
        await this.session.sendCustomMessage(
          {
            customType: PLAN_NOTE_TYPE,
            content: [{ type: "text", text }],
            display: true,
            details: { source: "user" },
          },
          { triggerTurn: wake },
        );
      },
      log,
    });
    this.id = manager.getSessionId();
    this.gate.sessionId = this.id;
    this.subscribe();
  }

  static async create(cwd: string, log: HarnessLog, options: PiSessionOptions = {}): Promise<PiHarnessSession> {
    // Issue #56: a client-minted id is honored, so the session file — cast
    // on the first message — carries the id the client already named its
    // view, sheet and container after. pi 1.5.0 validates the charset too.
    const manager = SessionManager.create(cwd, undefined, options.sessionId ? { id: options.sessionId } : undefined);
    // A new session has no history; nothing to repair (B2).
    return PiHarnessSession.build(manager, cwd, log, options, false);
  }

  static async open(path: string, cwd: string, log: HarnessLog, options: PiSessionOptions = {}): Promise<PiHarnessSession> {
    const manager = SessionManager.open(path);
    return PiHarnessSession.build(manager, cwd, log, options, true);
  }

  private static async build(
    manager: SessionManager,
    cwd: string,
    log: HarnessLog,
    options: PiSessionOptions,
    repairTail: boolean,
  ): Promise<PiHarnessSession> {
    const agentDir = options.agentDir ?? getAgentDir();
    // B2: fix a tail left dangling by a crash mid-interactive-tool BEFORE the
    // session is created from it, so the restored transcript is valid (empty
    // registry until A1 → no-op today).
    if (repairTail) {
      const repaired = repairDanglingInteractiveCalls(manager, INTERACTIVE_TOOLS);
      if (repaired > 0) log(`repaired ${repaired} dangling interactive call(s) in session ${manager.getSessionId()}`);
    }
    // Built-in extensions: the user's `disabledExtensions` decides which ones
    // compose, and an enabled extension's skills ride along. Read once per
    // build; a settings change lands on the next session/new|load (no hot
    // reload — see docs/pi-acp-IMPLEMENTATION.md §18).
    const zuSettings = await readZuSettings(agentDir);
    const gate = new PermissionGate(log);
    const captainId = manager.getSessionId();
    // A1: the ask tool waits through B2's AnswerRegistry; the ref is filled
    // once the registry exists below (the extension captures it by identity).
    const askBridge: { current: AskBridge | null } = { current: null };
    // A2: the plan tools and the `_zu/todos/*` channel share one store keyed
    // by this session's id, so the entry is built per session like ask's.
    const composed = composeExtensions(
      [
        ...ZU_EXTENSIONS,
        askExtensionEntry(askBridge),
        todosExtensionEntry(captainId, agentDir),
        visualizeExtensionEntry(),
      ],
      zuSettings,
    );
    const todosDir = todosRoot(agentDir);
    const sessionRef: { session: PiAgentSession | null } = { session: null };
    const trustRef = { trusted: options.trusted ?? false };
    const subagentManager = new SubagentManager(
      subagentHost(cwd, manager, sessionRef, log, agentDir, () => trustRef.trusted),
    );
    const built = await createPiServices({
      cwd,
      agentDir,
      ...(options.trusted === undefined ? {} : { trusted: options.trusted }),
      ...(options.elicit ? { elicit: options.elicit } : {}),
      ...(options.mcpServers ? { mcpServers: options.mcpServers } : {}),
      extensionFactories: [
        ...hostExtensionFactories(gate, subagentManager, captainId, log, () =>
          typeof (sessionRef.session as any)?.isIdle === "function"
            ? Boolean((sessionRef.session as any).isIdle())
            : Boolean((sessionRef.session as any)?.isIdle),
        ),
        ...composed.factories,
      ],
      additionalSkillPaths: composed.skillDirs,
      additionalExtensionPaths: extraExtensionPaths(),
      noExtensions: extensionsDisabled(),
      log,
    });
    trustRef.trusted = built.trusted;
    const created = await createAgentSessionFromServices({
      services: built.services,
      sessionManager: manager,
      // The Bash tool's raw output feeds the session's title channel
      // (issue #73); the custom definition overrides the built-in by name
      // and keeps the user's shell settings and the renderer.
      customTools: [
        tappedBashTool(cwd, built.services.settingsManager),
      ],
    });
    sessionRef.session = created.session;
    const answerRegistry = new AnswerRegistry(manager, INTERACTIVE_TOOLS, {
      log,
      inject: async (text, meta) => {
        // A late answer enters the conversation as its own display message
        // and wakes the model so it reacts to the answer (B2).
        await created.session.sendCustomMessage(
          {
            customType: TOOL_RESPONSE_TYPE,
            content: [{ type: "text", text }],
            display: true,
            details: meta,
          },
          { triggerTurn: true },
        );
      },
    });
    // A1: the ask tool's wait = B2 announce + the `_zu/ask/create` wire
    // request, both settling through the registry so every path (live answer,
    // wire answer, supersede, client drop) writes its receipt exactly once.
    askBridge.current = {
      ask: (request) =>
        new Promise((resolve) => {
          answerRegistry.announce(
            request.toolCallId,
            { questions: request.questions },
            resolve as (response: unknown) => void,
          );
          const client = gate.bridge;
          if (!client?.ask) {
            // Defensive: attachClient only installs the bridge with an ask
            // face, so this means no client — don't strand the waiter.
            void answerRegistry.answer(request.toolCallId, { action: "cancel" });
            return;
          }
          client
            .ask({ ...request, sessionId: manager.getSessionId() })
            .then((response) => answerRegistry.answer(request.toolCallId, response))
            .catch(() => answerRegistry.answer(request.toolCallId, { action: "cancel" }));
        }),
    };
    const rec = new PiHarnessSession(
      manager,
      created.session,
      cwd,
      log,
      gate,
      built.services.settingsManager,
      subagentManager,
      answerRegistry,
      todosDir,
      !zuSettings.disabled.has("todos"),
    );
    try {
      await created.session.bindExtensions({
        uiContext: rec.uiContext(),
        mode: "rpc",
        abortHandler: () => {
          void created.session.abort();
        },
        onError: (failure) => {
          log(`extension error in ${rec.id}: ${failure.event}: ${String(failure.error)}`);
        },
      });
    } catch (error) {
      log(`bindExtensions failed for ${rec.id}: ${error instanceof Error ? error.message : String(error)}`);
    }
    return rec;
  }

  async prompt(text: string): Promise<StopReason> {
    this.lastStop = undefined;
    // A user-typed message supersedes every waiting interactive tool (B2);
    // they resolve with the supersede sentinel and answer with the ack.
    this.answerRegistry.supersedeAll();
    // `/compact` is the one built-in the distro executes itself; extension
    // commands, templates and skills are handled by `session.prompt`.
    const trimmed = text.trim();
    if (trimmed === "/compact" || trimmed.startsWith("/compact ")) {
      await this.session.compact(trimmed.slice("/compact".length).trim() || undefined);
      this.emitUsage();
      return "end_turn";
    }
    await this.session.prompt(text);
    this.emitUsage();
    return stopReasonFromPi(this.lastStop);
  }

  async abort(): Promise<void> {
    await this.session.abort();
  }

  dispose(): void {
    try {
      this.session.dispose();
    } catch {
      // Already disposed.
    }
  }

  /** The load replay, mapped from pi's own projection of the session file. */
  /** The ACP extension surface for subagents (`_zu/subagents/*`). */
  get subagents() {
    return {
      list: (filter: { captainId?: string } = {}) =>
        this.subagentManager.list({ ...filter, captainId: filter.captainId ?? this.id }),
      transcript: (runId: string) => this.subagentManager.transcript(runId),
      dispatch: (agent: string, task: string) => this.subagentManager.spawn(this.id, agent, task),
      abort: (runId: string) => this.subagentManager.abort(runId),
    };
  }

  /** The interactive-tool mailbox for `_zu/ask/*` (B2). */
  get answers() {
    return this.answerRegistry;
  }

  /**
   * The session plan surface for `_zu/todos/*` (A2): the same store the
   * `todo_*` tools use, so a panel edit is the model's next `todo_list` and
   * a tool's write is the panel's next poll. Unconditional — the panel works
   * even when the user turned the `todos` extension off.
   *
   * A successful `write` is a user edit (the panel is this surface's only
   * caller — the model writes through its tools), so it also steers: the
   * change becomes a note in the conversation and wakes the model to
   * re-read; a removal interrupts a running turn first, so the model cannot
   * finish work the user just took away (see ../todos/steer.ts). When the
   * `todos` extension is disabled there is no `todo_list` to re-read, so a
   * write only lands in the file and the entry advertises that with
   * `enabled: false`.
   */
  get todos() {
    return {
      enabled: this.planToolsEnabled,
      list: () => readPlan(this.id, this.todosDir),
      write: (op: unknown) => {
        const before = readPlan(this.id, this.todosDir);
        const result = applyTodoOp(this.id, this.todosDir, op);
        if (result.ok && this.planToolsEnabled) {
          void this.planSteer.afterEdit(planEditFacts(op, before));
        }
        return result;
      },
    };
  }

  configOptions(): SessionConfigOption[] {
    return sessionConfigOptions(
      this.session.modelRuntime as ConfigOptionsRuntime | undefined,
      this.settingsManager.getEnabledModels(),
      this.session.model,
      this.session.getAvailableThinkingLevels(),
      this.session.thinkingLevel,
    );
  }

  availableCommands(): AvailableCommand[] {
    const commands = this.session.extensionRunner.getRegisteredCommands().map((command) => ({
      name: command.invocationName,
      description: command.description ?? "",
    }));
    const templates = this.session.promptTemplates.map((template) => ({
      name: template.name,
      description: template.description ?? "",
    }));
    const skills = this.session.resourceLoader.getSkills().skills.map((skill) => ({
      name: `skill:${skill.name}`,
      description: skill.description ?? "",
    }));
    return [
      { name: "compact", description: "Compact the conversation context." },
      ...commands,
      ...templates,
      ...skills,
    ];
  }

  /**
   * The session banner (issue #20 §5.3): what the transcript's first line
   * needs — the pinned pi version, the session's model and thinking level,
   * and the loaded resources with their load diagnostics. `undefined` means
   * "no banner": the client draws none rather than a fake empty one.
   */
  banner(): SessionBanner | undefined {
    const loader = this.session.resourceLoader;
    const model = this.session.model as { provider?: string; id?: string; name?: string } | undefined;
    const bannerModel =
      model?.provider && model.id
        ? { provider: model.provider, id: model.id, ...(model.name ? { name: model.name } : {}) }
        : undefined;
    const skills = loader.getSkills();
    const extensions = loader.getExtensions();
    const prompts = loader.getPrompts();
    const themes = loader.getThemes();
    const warnings: SessionBannerWarning[] = [
      ...diagnosticsToWarnings(skills.diagnostics, "skill"),
      ...diagnosticsToWarnings(prompts.diagnostics, "prompt"),
      ...diagnosticsToWarnings(themes.diagnostics, "theme"),
      ...extensions.errors.map((failure) => ({
        severity: "error" as const,
        source: "extension" as const,
        message: `${failure.path}: ${failure.error}`,
      })),
      ...(extensions.warnings ?? []).map((warning) => ({
        severity: "warning" as const,
        source: "extension" as const,
        message: `${warning.path}: ${warning.warning}`,
      })),
    ];
    return {
      version: PI_VERSION,
      ...(bannerModel ? { model: bannerModel } : {}),
      ...(this.session.thinkingLevel ? { thinkingLevel: this.session.thinkingLevel } : {}),
      skills: skills.skills.map((skill) => ({
        name: skill.name,
        ...(skill.description ? { description: skill.description } : {}),
        source: skill.sourceInfo.source,
      })),
      // The banner names what the user's configuration loads. pi's own CLI
      // builtins are `hidden` (that is what the flag is for), and the host's
      // anonymous inline plumbing (`<inline:N>` — the always-on gate /
      // prompt-section / dispatch pair) is not a user-facing extension; the
      // registry's named `zu:*` entries stay listed (the builtin-extension
      // registry, IMPLEMENTATION §18).
      extensions: extensions.extensions
        .filter((extension) => !extension.hidden && !isAnonymousInline(extension.path))
        .map((extension) => ({
          name: extensionName(extension.path),
          path: extension.path,
          source: extension.sourceInfo.source,
        })),
      warnings,
    };
  }

  async setConfigOption(configId: string, value: string): Promise<SessionConfigOption[]> {
    if (configId === "model") {
      const [provider, ...rest] = value.split("/");
      const id = rest.join("/");
      const model = provider && id ? this.session.modelRuntime.getModel(provider, id) : undefined;
      if (!model) {
        throw codedError(-32602, "unknown_model", `unknown model ${value}`);
      }
      await this.session.setModel(model, { persist: false });
      this.emitConfig();
      return this.configOptions();
    }
    if (configId === "thought_level") {
      // Issue #54: pi clamps the level to the model's own ladder
      // (`getAvailableThinkingLevels`), so an off-menu value settles on the
      // nearest supported level instead of erroring with
      // `invalid_thought_level` — the composer's menus only offer levels the
      // session announced, and pi owns the truth about what applies.
      this.session.setThinkingLevel(value as ThinkingLevel, { persist: false });
      this.emitConfig();
      return this.configOptions();
    }
    throw codedError(-32602, "unknown_config_option", `unknown config option ${configId}`);
  }

  modeState(): SessionModeState {
    return {
      currentModeId: this.gate.mode,
      availableModes: APPROVAL_MODES.availableModes,
    };
  }

  async setMode(modeId: string): Promise<void> {
    if (!APPROVAL_MODES.availableModes.some((mode) => mode.id === modeId)) {
      throw codedError(-32602, "unknown_mode", `unknown mode ${modeId}`);
    }
    this.gate.mode = modeId;
    this.emit({ sessionUpdate: "current_mode_update", currentModeId: modeId });
  }

  attachClient(bridge: HarnessClientBridge | null): void {
    this.gate.bridge = bridge;
  }

  private emitConfig(): void {
    this.emit({ sessionUpdate: "config_option_update", configOptions: this.configOptions() });
  }

  private emitUsage(): void {
    const usage = this.session.getContextUsage();
    if (!usage || usage.tokens === null) return;
    this.emit({ sessionUpdate: "usage_update", used: usage.tokens, size: usage.contextWindow });
  }

  replay(): SessionUpdate[] {
    const projection = this.manager.buildSessionProjection();
    const entries: ReplayEntry[] = projection.entries.map((projected) => ({
      sourceId: projected.sourceEntry.id,
      timestamp: projected.sourceEntry.timestamp,
      messages: projected.messages.map((message) => message as ReplayMessageShape),
    }));
    return replayUpdates(entries);
  }

  onUpdate(push: (update: SessionUpdate) => void): void {
    this.push = push;
  }

  private emit(update: SessionUpdate | undefined): void {
    if (update) this.push?.(update);
  }

  private nextMessageId(): string {
    this.messageSeq += 1;
    return `m${this.messageSeq}`;
  }

  /** The anchor facts the streaming chunks of the current message carry. */
  private chunkMeta(): PiMeta | undefined {
    if (this.currentMessageAt === undefined && !this.currentModel) return undefined;
    return {
      ...(this.currentMessageAt !== undefined ? { at: this.currentMessageAt } : {}),
      ...(this.currentModel ? { model: this.currentModel } : {}),
    };
  }

  /**
   * The session entry a message just persisted, read back after the write:
   * the user's own prompt (its chunk lands with the entry's id, re-keying the
   * client's optimistic row) or the assistant's settled message (the settle
   * marker, `_meta.pi` only).
   *
   * pi 1.5.0 persists a message at `message_end` and never announces a
   * regular message with `entry_appended` (that event covers custom entries
   * and boundary drafts only), so the entry is read back by the branch
   * position the append landed at — one microtask after the event, when the
   * write has happened but before the run moves on.
   */
  private emitPersistedMessage(index: number, role: "user" | "assistant"): void {
    const entry = this.manager.getBranch()[index];
    if (!entry || entry.type !== "message" || typeof entry.id !== "string") return;
    const message = entry.message as {
      role?: string;
      content?: unknown;
      stopReason?: string;
      errorMessage?: string;
    };
    if (message?.role !== role) return;
    const parsed = typeof entry.timestamp === "string" ? Date.parse(entry.timestamp) : NaN;
    const at = Number.isFinite(parsed) ? parsed : Date.now();
    if (role === "user") {
      for (const update of userEntryUpdates(entry.id, message.content, at)) this.emit(update);
      return;
    }
    const durationMs = this.currentMessageAt !== undefined ? Math.max(0, at - this.currentMessageAt) : undefined;
    this.emit(
      settledUpdate({
        entryId: entry.id,
        at,
        ...(durationMs !== undefined ? { durationMs } : {}),
        ...(this.currentModel ? { model: this.currentModel } : {}),
        ...(message.stopReason !== undefined ? { stopReason: stopReasonFromPi(message.stopReason) } : {}),
        ...(message.errorMessage ? { error: message.errorMessage } : {}),
      }),
    );
  }

  /** pi events in, ACP update payloads out — synchronously, preserving order. */
  private subscribe(): void {
    this.session.subscribe((event: AgentSessionEvent) => {
      switch (event.type) {
        case "message_start": {
          if ((event.message as { role?: string })?.role === "assistant") {
            this.currentMessageId = this.nextMessageId();
            this.currentMessageAt = Date.now();
            const model = this.session.model as
              | { provider?: string; id?: string; name?: string }
              | undefined;
            this.currentModel =
              model?.provider && model.id
                ? {
                    provider: model.provider,
                    id: model.id,
                    ...(model.name ? { name: model.name } : {}),
                  }
                : undefined;
          }
          return;
        }
        case "message_update": {
          this.emit(assistantDeltaUpdate(event.assistantMessageEvent, this.currentMessageId, this.chunkMeta()));
          return;
        }
        case "message_end": {
          const message = event.message as {
            role?: string;
            stopReason?: string;
            errorMessage?: string;
          };
          // The entry lands in the file right after this event, so its id is
          // only readable a microtask later (see emitPersistedMessage).
          if (message?.role === "user" || message?.role === "assistant") {
            const index = this.manager.getBranch().length;
            const role = message.role;
            queueMicrotask(() => this.emitPersistedMessage(index, role));
          }
          if (message?.role !== "assistant") return;
          // Track the turn's final stop reason; the prompt reply reads it.
          // A provider error rides the settle marker's `_meta.pi` — it is
          // never stuffed into a text chunk (issue #20 §5.1).
          this.lastStop = message.stopReason;
          return;
        }
        case "entry_appended": {
          // A custom entry or a boundary draft: no transcript row has a
          // shape for it (pi announces regular messages from message_end,
          // above). Agent-side state, not conversation.
          return;
        }
        case "tool_execution_start": {
          if (event.parentToolCallId) return; // a nested call; its parent card carries the run
          this.argsByToolCall.set(event.toolCallId, event.args);
          this.emit(toolCallStartUpdate(event.toolCallId, event.toolName, event.args));
          return;
        }
        case "tool_execution_update": {
          if (event.parentToolCallId) return;
          this.emit(toolCallPartialUpdate(event.toolCallId, event.partialResult));
          return;
        }
        case "tool_execution_end": {
          if (event.parentToolCallId) return;
          const args = this.argsByToolCall.get(event.toolCallId);
          this.argsByToolCall.delete(event.toolCallId);
          this.emit(
            toolCallEndUpdate(
              event.toolCallId,
              event.toolName,
              event.result,
              event.isError,
              args,
              toolStats(event.durationMs, event.result),
            ),
          );
          return;
        }
        case "thinking_level_changed": {
          this.emitConfig();
          return;
        }
        case "session_info_changed": {
          this.emit({ sessionUpdate: "session_info_update", title: event.name ?? null });
          return;
        }
        case "compaction_start": {
          this.compactionSeq += 1;
          this.currentCompactionId = `c${this.compactionSeq}`;
          this.emit({
            sessionUpdate: "compaction_update",
            compactionId: this.currentCompactionId,
            status: "in_progress",
          });
          return;
        }
        case "compaction_end": {
          const id = this.currentCompactionId || `c${++this.compactionSeq}`;
          const status = event.aborted ? "cancelled" : event.errorMessage ? "failed" : "completed";
          this.emit({
            sessionUpdate: "compaction_update",
            compactionId: id,
            status,
            ...(event.result?.summary ? { summary: [{ type: "text", text: event.result.summary }] } : {}),
            ...(event.errorMessage ? { error: event.errorMessage } : {}),
          });
          this.emitUsage();
          return;
        }
        case "agent_settled": {
          // Plan notes queued while the turn ran go out now. A settle the
          // user caused (abort) flushes without waking: the user said stop,
          // and a plan note must not overrule that by starting a turn.
          void this.planSteer.flush(!event.aborted);
          return;
        }
        default:
          // Turn lifecycle, retries, queue updates: no P0 update
          // shape (v2 §5.0 moves them to core events in later stages).
          return;
      }
    });
  }

  /**
   * The extension UI bridge: confirm/select/input/editor are an ACP
   * elicitation form; notify/status/title become notices and session info.
   * The rich presentation surfaces ACP has no equivalent for degrade to a
   * warning notice, which the client renders in the transcript.
   */
  private uiContext(): ExtensionUIContext {
    const unsupported = (method: string) => (..._args: unknown[]) => {
      this.log(`extension ui ${method} is not supported over ACP`);
      this.emit({
        sessionUpdate: "notice",
        notice: {
          severity: "warning",
          title: `${method} is not supported`,
          description: "This extension UI surface has no ACP equivalent yet.",
        },
      } as unknown as SessionUpdate);
      return undefined;
    };
    const form = async (
      message: string,
      properties: Record<string, unknown>,
      required: string[],
    ): Promise<{ action?: string; content?: Record<string, unknown> } | undefined> => {
      if (!this.gate.bridge) {
        this.log("extension ui requested without a client bridge");
        return undefined;
      }
      try {
        return (await this.gate.bridge.elicit({
          mode: "form",
          sessionId: this.id,
          message,
          requestedSchema: { type: "object", properties, required },
        })) as { action?: string; content?: Record<string, unknown> };
      } catch (error) {
        this.log(`extension ui elicitation failed: ${String(error)}`);
        return undefined;
      }
    };
    return {
      select: async (title, options) => {
        const result = await form(title, {
          choice: { type: "string", title, enum: options },
        }, ["choice"]);
        if (result?.action !== "accept") return undefined;
        const value = result.content?.choice;
        return typeof value === "string" ? value : undefined;
      },
      confirm: async (title, message) => {
        const result = await form(`${title}\n${message}`, {
          confirmed: { type: "boolean", title, description: message },
        }, ["confirmed"]);
        return result?.action === "accept" && result.content?.confirmed === true;
      },
      input: async (title, placeholder) => {
        const result = await form(title, {
          value: { type: "string", title, description: placeholder ?? "" },
        }, ["value"]);
        if (result?.action !== "accept") return undefined;
        const value = result.content?.value;
        return typeof value === "string" ? value : undefined;
      },
      editor: async (title, prefill) => {
        const result = await form(title, {
          value: { type: "string", title, description: "Markdown or plain text", default: prefill ?? "" },
        }, ["value"]);
        if (result?.action !== "accept") return undefined;
        const value = result.content?.value;
        return typeof value === "string" ? value : undefined;
      },
      notify: (message, type) => {
        this.emit({
          sessionUpdate: "notice",
          notice: {
            severity: type === "error" ? "error" : type === "warning" ? "warning" : "info",
            title: message,
          },
        } as unknown as SessionUpdate);
      },
      setStatus: (key, text) => {
        this.emit({
          sessionUpdate: "notice",
          notice: {
            severity: "info",
            title: `${key}${text ? `: ${text}` : ""}`,
          },
        } as unknown as SessionUpdate);
      },
      setTitle: (title) => {
        this.emit({ sessionUpdate: "session_info_update", title });
      },
      setWorkingMessage: unsupported("setWorkingMessage") as never,
      setWorkingVisible: unsupported("setWorkingVisible") as never,
      setWorkingIndicator: unsupported("setWorkingIndicator") as never,
      setHiddenThinkingLabel: unsupported("setHiddenThinkingLabel") as never,
      setWidget: unsupported("setWidget") as never,
      setFooter: unsupported("setFooter") as never,
      setHeader: unsupported("setHeader") as never,
      custom: unsupported("custom") as never,
      pasteToEditor: unsupported("pasteToEditor") as never,
      setEditorText: unsupported("setEditorText") as never,
      addAutocompleteProvider: unsupported("addAutocompleteProvider") as never,
      setEditorComponent: unsupported("setEditorComponent") as never,
      onTerminalInput: () => {
        this.log("extension ui onTerminalInput is not supported over ACP");
        return () => {};
      },
      getEditorText: () => "",
      getEditorComponent: () => undefined,
      getToolsExpanded: () => false,
      setToolsExpanded: unsupported("setToolsExpanded") as never,
      getAllThemes: () => [],
      getTheme: () => undefined,
      setTheme: () => ({ success: false, error: "the client owns its theme" }),
      theme: undefined as never,
    } as ExtensionUIContext;
  }
}

/** The projection message shape the replay reads; a pi AgentMessage view. */
type ReplayMessageShape = import("./replay").ReplayMessage;

/**
 * pi's services for one cwd, with conservative project trust: the saved
 * decision stands, an unsaved one stays untrusted (and logged) — project
 * resources stay unloaded instead of silently loading.
 */
/** The subagent manager's host: this session is its captain; settled runs
 * live in `~/.steer/subagents/index.json`. */
function subagentHost(
  cwd: string,
  manager: SessionManager,
  sessionRef: { session: PiAgentSession | null },
  log: HarnessLog,
  agentDir: string,
  trusted: () => boolean,
): SubagentHost {
  const root = join(zuStateDir(agentDir), "subagents");
  const index = join(root, "index.json");
  const readIndex = (): SubagentRun[] => {
    try {
      const parsed = JSON.parse(readFileSync(index, "utf8")) as { runs?: SubagentRun[] };
      return Array.isArray(parsed.runs) ? parsed.runs : [];
    } catch {
      return [];
    }
  };
  return {
    agentDir,
    log: (message) => log(message),
    captainFor: (id) =>
      sessionRef.session
        ? {
            id,
            path: manager.getSessionFile() ?? "",
            cwd,
            manager,
            session: sessionRef.session,
          }
        : undefined,
    settledRuns: readIndex,
    saveSettled: (run) => {
      try {
        mkdirSync(root, { recursive: true });
        const runs = readIndex().filter((entry) => entry.runId !== run.runId).concat(run);
        const temp = `${index}.tmp`;
        writeFileSync(temp, JSON.stringify({ v: 1, runs }, null, 2));
        renameSync(temp, index);
      } catch (error) {
        log(`subagent index write failed: ${String(error)}`);
      }
    },
    appendCaptainNote: (id, kind, text, data) => {
      try {
        manager.appendCustomEntry(kind === "dispatch" ? "zu.dispatch" : "zu.delivery", { ...(data as object), text });
      } catch (error) {
        log(`captain note failed for ${id}: ${String(error)}`);
      }
    },
    emit: (type, _data) => log(`subagent ${type}`),
    projectTrusted: () => trusted(),
    isArchived: () => false,
  };
}

/** The HarnessPorts over real pi sessions. */
export interface PiPortsOptions {
  agentDir?: string;
}

/**
 * The id shape `session/new` `_meta.sessionId` must have (issue #56): a
 * UUID. The app mints v4 and pi itself mints v7, so both this app's
 * client-minted ids and a harness-minted id from an older build rebuild the
 * same way; anything else is refused before pi sees it. The occupancy check
 * walks the real session directory, so a client can never shadow an
 * existing session file — including one the UI's own scan has not seen yet.
 */
const CLIENT_SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function assertFreeSessionId(id: string, agentDir = getAgentDir()): void {
  if (!CLIENT_SESSION_ID.test(id)) {
    throw codedError(JSONRPC_CODE.invalidParams, "invalid_session_id", `session id ${id} is not a UUID`);
  }
  if (findPiSessionFile(id, agentDir)) {
    throw codedError(
      JSONRPC_CODE.invalidParams,
      "session_id_taken",
      `session ${id} already has a session file`,
      { sessionId: id },
    );
  }
}

function optionsFor(context: HarnessSessionContext | undefined, base: PiPortsOptions): PiSessionOptions {
  return {
    ...(base.agentDir ? { agentDir: base.agentDir } : {}),
    ...(context?.elicit ? { elicit: context.elicit } : {}),
    ...(context?.mcpServers ? { mcpServers: context.mcpServers as AcpMcpServer[] } : {}),
    ...(context?.sessionId ? { sessionId: context.sessionId } : {}),
  };
}

export function createPiPorts(log: HarnessLog, base: PiPortsOptions = {}): HarnessPorts {
  return {
    createSession: (cwd: string, context?: HarnessSessionContext) => {
      if (context?.sessionId) assertFreeSessionId(context.sessionId, base.agentDir);
      return PiHarnessSession.create(cwd, log, optionsFor(context, base));
    },
    resumeSession: async (sessionId: string, cwd: string, context?: HarnessSessionContext) => {
      return createPiPorts(log, base).loadSession(sessionId, cwd, context);
    },
    loadSession: async (sessionId: string, cwd: string, context?: HarnessSessionContext) => {
      const path = findPiSessionFile(sessionId, base.agentDir);
      if (!path) {
        throw codedError(-32002, "session_not_found", `no session file for ${sessionId} under ${join(base.agentDir ?? getAgentDir(), "sessions")}`, {
          sessionId,
        });
      }
      const manager = SessionManager.open(path);
      const sessionCwd = manager.getCwd();
      if (sessionCwd !== cwd) {
        throw codedError(
          -32602,
          "cwd_mismatch",
          `session ${sessionId} belongs to ${sessionCwd}, not ${cwd}`,
          { sessionId },
        );
      }
      return PiHarnessSession.open(path, cwd, log, optionsFor(context, base));
    },
  };
}

/** One resource diagnostic → a banner warning, tagged with its loader. */
function diagnosticsToWarnings(
  diagnostics: Array<{ type: string; message: string; path?: string }>,
  source: SessionBannerWarning["source"],
): SessionBannerWarning[] {
  return diagnostics.map((diagnostic) => ({
    severity: diagnostic.type === "error" ? "error" : "warning",
    source,
    message: diagnostic.path ? `${diagnostic.path}: ${diagnostic.message}` : diagnostic.message,
  }));
}

/** The name an extension row shows: its file's stem. */
function extensionName(path: string): string {
  const base = path.split("/").pop() ?? path;
  const stem = base.replace(/\.(ts|js|mjs|cjs)$/i, "");
  return stem || base;
}

/**
 * An inline factory the host binds itself, without a registry name: the
 * always-on gate, prompt sections, dispatch. The banner's extension list is
 * the user's load report, so these stay out; a registry entry (`zu:*`) is a
 * named, disable-able extension and stays in.
 */
function isAnonymousInline(path: string): boolean {
  return path.startsWith("<inline:") && !path.startsWith("<inline:zu:");
}

/**
 * Resolves a session id to its file under `~/.pi/agent/sessions/` by file
 * name (pi names files `<timestamp>_<sessionId>.jsonl`). A bounded walk —
 * no full listing, no network.
 */
export function findPiSessionFile(sessionId: string, agentDir = getAgentDir()): string | undefined {
  const root = join(agentDir, "sessions");
  let found: string | undefined;
  let foundMtime = 0;
  const suffix = `_${sessionId}.jsonl`;
  const visit = (dir: string, depth: number): void => {
    if (depth > 3) return;
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        visit(path, depth + 1);
      } else if (entry.name.endsWith(suffix)) {
        try {
          const mtime = statSync(path).mtimeMs;
          if (mtime > foundMtime) {
            foundMtime = mtime;
            found = path;
          }
        } catch {
          // Removed while walking.
        }
      }
    }
  };
  visit(root, 0);
  return found;
}
