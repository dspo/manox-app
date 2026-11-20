/**
 * The ACP agent-side method surface, served on the official SDK
 * (`@agentclientprotocol/sdk`, stable ACP v1).
 *
 * The SDK owns the wire: ndjson framing, JSON-RPC id pairing, batch-array
 * rejection, and error responses. This module owns the semantics:
 *
 *   initialize            → protocolVersion + capabilities
 *   session/new           → one pi session; configOptions + modes
 *   session/load          → replay the file, then config/modes
 *   session/resume        → reattach without replay
 *   session/close         → cancel and free the session
 *   session/set_config_option → model / thought level
 *   session/set_mode      → approval policy
 *   session/prompt        → one turn; settle = stopReason
 *   session/cancel        → abort; pending prompt answers cancelled
 *   _zu/subagents/*       → subagent runs (B3)
 *   _zu/ask/pending       → restart-surviving question cards (B2)
 *   _zu/ask/answer        → resolve a live wait or inject a late answer (B2)
 *   _zu/todos/list|write  → the session plan the panel edits (A2)
 *   _zu/ask/create        → the model's structured questions (A1, server→client)
 *
 * This file must not import pi. The pi glue sits behind `HarnessPorts`
 * (src/acp/pi-session.ts), which keeps `bun test` free of pi and of network.
 * The permission/elicitation bridge is a server→client request owned by the
 * session object; the app only supplies the client context.
 */

import type { AskRequest, AskResponse } from "../extensions/ask";
import {
  PROTOCOL_VERSION,
  agent,
  type AgentApp,
  type AgentContext,
  type AvailableCommand,
  type ContentBlock,
  type CreateElicitationRequest,
  type CreateElicitationResponse,
  type LoadSessionRequest,
  type NewSessionRequest,
  type RequestPermissionRequest,
  type RequestPermissionResponse,
  type SessionConfigOption,
  type SessionModeState,
  type SessionNotification,
  type SessionUpdate,
  type StopReason,
} from "@agentclientprotocol/sdk";
import { codedError, JSONRPC_CODE, normalizeError, sessionNotFoundError } from "./errors";
import { PI_VERSION } from "../version";
import { promptBlocksToText } from "./mapping";
import { EMPTY_PLAN, TODO_FAILURE_TEXT, type TodoFile, type TodoOpResult } from "../todos/core";
import type { TrustElicit } from "../pi/trust";

/** Server→client requests a session may make while a turn runs (or between
 * turns): tool approval and structured user input. */
export interface HarnessClientBridge {
  requestPermission(params: RequestPermissionRequest): Promise<RequestPermissionResponse>;
  elicit(params: CreateElicitationRequest): Promise<CreateElicitationResponse>;
  /** A1: `_zu/ask/create` — the ask_user_question card request. */
  ask?(request: AskRequest): Promise<AskResponse>;
}

/** What the adapter needs from a live session. Implemented by the pi glue. */
export interface HarnessSession {
  /** pi's session id; doubles as the ACP sessionId. */
  readonly id: string;
  /** Runs one turn; resolves with the ACP stop reason when it settles. */
  prompt(text: string): Promise<StopReason>;
  /** Stops the running turn; the pending prompt then resolves `cancelled`. */
  abort(): Promise<void>;
  /** Releases pi's resources; the session file on disk stays. */
  dispose(): void;
  /** The load replay: this session's history as update payloads. */
  replay(): SessionUpdate[];
  /** Live update payloads, pushed synchronously and in pi's event order. */
  onUpdate(push: (update: SessionUpdate) => void): void;
  /** The current configuration selectors (model, thought level). */
  configOptions(): SessionConfigOption[];
  /** Commands the session can execute from a prompt (`/name args`). */
  availableCommands(): AvailableCommand[];
  /** The transcript's first-line facts, when the session has any. */
  banner?(): SessionBanner | undefined;
  /** Applies one config value; returns the fresh option list. */
  setConfigOption(configId: string, value: string): Promise<SessionConfigOption[]>;
  /** The approval-policy modes. */
  modeState(): SessionModeState;
  /** Selects one mode; the session emits `current_mode_update` itself. */
  setMode(modeId: string): Promise<void>;
  /** Gives the session a way to call back into the client. */
  attachClient(bridge: HarnessClientBridge | null): void;
  /** The subagent surface (the `_zu/subagents/*` extension methods). */
  subagents?: {
    list(filter?: { captainId?: string }): any[];
    transcript(runId: string): any;
    dispatch(agent: string, task: string): any;
    abort(runId: string): Promise<void>;
  };
  /** The interactive-tool mailbox (B2): late answers and restart-pending asks. */
  answers?: {
    answer(toolCallId: string, response: unknown): Promise<{ delivered: "live" | "late" | "none" }>;
    pending(): Array<{ toolCallId: string; args: unknown }>;
  };
  /** The session plan surface (`_zu/todos/*`, A2). The store is shared with
   * the `todo_*` tools, so a panel edit and a tool read see one file. */
  todos?: {
    /** Whether the model has the matching `todo_*` tools registered. */
    enabled?: boolean;
    list(): TodoFile;
    write(op: unknown): TodoOpResult;
  };
}

/** What the ACP request carries into session construction. */
export interface HarnessSessionContext {
  /** Trust is asked while `session/new` is still in flight. */
  elicit?: TrustElicit;
  /** `session/new|load` servers; the distro hands them to pi's MCP extension. */
  mcpServers?: unknown[];
  /** The client's minted session id (`session/new` `_meta.sessionId`, issue
   * #56): the port validates it (a UUID, unoccupied) and pi honors it, so
   * the ACP sessionId is the client's handle from before any process
   * exists. Absent: the harness mints, as any standard client expects. */
  sessionId?: string;
}

/** How the method surface obtains sessions (pi in production, fakes in tests). */
export interface HarnessPorts {
  createSession(cwd: string, context?: HarnessSessionContext): Promise<HarnessSession>;
  loadSession(sessionId: string, cwd: string, context?: HarnessSessionContext): Promise<HarnessSession>;
  /** Reattach without replay; defaults to `loadSession` when absent. */
  resumeSession?(sessionId: string, cwd: string, context?: HarnessSessionContext): Promise<HarnessSession>;
}

/** One resource warning in the session banner (a load diagnostic). */
export interface SessionBannerWarning {
  severity: "warning" | "error";
  source: "skill" | "extension" | "prompt" | "theme";
  message: string;
}

/**
 * The `_zu/session_banner` payload (issue #20 §5.3): what the transcript's
 * first line needs — the pinned pi version, the session's model and thinking
 * level, and the loaded resources. Sent after `available_commands_update` on
 * `session/new|load|resume`; a session without one simply sends nothing.
 */
export interface SessionBanner {
  version: string;
  model?: { provider: string; id: string; name?: string };
  thinkingLevel?: string;
  skills: Array<{ name: string; description?: string; source: string }>;
  extensions: Array<{ name: string; path: string; source: string }>;
  warnings: SessionBannerWarning[];
}

/** One stderr line; stdout is protocol-only. */
export type HarnessLog = (line: string) => void;

/**
 * Builds the agent app. Connect it with `app.connect(ndJsonStream(...))`.
 */
export function createAcpApp(ports: HarnessPorts, log: HarnessLog = () => {}): AgentApp {
  const sessions = new Map<string, HarnessSession>();
  const clients = new Map<string, AgentContext>();
  const busy = new Set<string>();
  const pumps = new Map<string, Promise<void>>();

  const app = agent({ name: "pi-harness" });

  app.onRequest("initialize", (cx) => {
    const requested = cx.params.protocolVersion;
    const protocolVersion = requested === PROTOCOL_VERSION ? requested : PROTOCOL_VERSION;
    return {
      protocolVersion,
      agentCapabilities: {
        loadSession: true,
        promptCapabilities: { image: false, audio: false, embeddedContext: false },
        sessionCapabilities: { resume: {}, close: {} },
      },
      authMethods: [],
      agentInfo: { name: "pi-harness", version: PI_VERSION },
    };
  });

  app.onRequest("session/new", mcpServersLenient<NewSessionRequest>, async (cx) => {
    void cx.params.additionalDirectories;
    // Issue #56: `_meta.sessionId` is the pairing extension that carries a
    // client-minted id; a standard client omits it and nothing changes.
    const requestedId = clientIdOf(cx.params);
    const context: HarnessSessionContext = {
      elicit: (request) => cx.client.request("elicitation/create", request as any),
      ...(requestedId ? { sessionId: requestedId } : {}),
      ...(cx.params.mcpServers && cx.params.mcpServers.length > 0 ? { mcpServers: cx.params.mcpServers } : {}),
    };
    const session = await withLog("session_new_failed", () => ports.createSession(cx.params.cwd, context));
    adopt(session, cx.client);
    await announceCommands(session);
    return {
      sessionId: session.id,
      modes: session.modeState(),
      configOptions: session.configOptions(),
    };
  });

  app.onRequest("session/load", mcpServersLenient<LoadSessionRequest>, async (cx) => {
    const context: HarnessSessionContext = {
      elicit: (request) => cx.client.request("elicitation/create", request as any),
      ...(cx.params.mcpServers && cx.params.mcpServers.length > 0 ? { mcpServers: cx.params.mcpServers } : {}),
    };
    const session = await withLog("session_load_failed", () =>
      ports.loadSession(cx.params.sessionId, cx.params.cwd, context),
    );
    adopt(session, cx.client);
    // ACP v1: the replay precedes the response, in order.
    for (const update of session.replay()) await notifyUpdate(session.id, update);
    await announceCommands(session);
    return { modes: session.modeState(), configOptions: session.configOptions() };
  });

  app.onRequest("session/resume", async (cx) => {
    const context: HarnessSessionContext = {
      elicit: (request) => cx.client.request("elicitation/create", request as any),
    };
    const session = await withLog("session_resume_failed", async () => {
      if (ports.resumeSession) return ports.resumeSession(cx.params.sessionId, cx.params.cwd, context);
      return ports.loadSession(cx.params.sessionId, cx.params.cwd, context);
    });
    adopt(session, cx.client);
    await announceCommands(session);
    return { modes: session.modeState(), configOptions: session.configOptions() };
  });

  app.onRequest("session/close", async (cx) => {
    const session = sessions.get(cx.params.sessionId);
    if (!session) return {};
    await session.abort().catch(() => {});
    session.dispose();
    sessions.delete(session.id);
    clients.delete(session.id);
    pumps.delete(session.id);
    return {};
  });

  app.onRequest("session/set_config_option", async (cx) => {
    const session = requireSession(cx.params.sessionId);
    const value = configValue(cx.params);
    const configOptions = await session.setConfigOption(cx.params.configId, value);
    return { configOptions };
  });

  app.onRequest("session/set_mode", async (cx) => {
    const session = requireSession(cx.params.sessionId);
    await session.setMode(cx.params.modeId);
    return {};
  });

  app.onRequest("_zu/subagents/list", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    return { runs: session.subagents?.list({ captainId: session.id }) ?? [] };
  });

  app.onRequest("_zu/subagents/transcript", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    if (!session.subagents) throw codedError(JSONRPC_CODE.methodNotFound, "subagents_unavailable", "no subagent surface");
    return session.subagents.transcript(cx.params.runId);
  });

  app.onRequest("_zu/subagents/dispatch", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    if (!session.subagents) throw codedError(JSONRPC_CODE.methodNotFound, "subagents_unavailable", "no subagent surface");
    return { run: session.subagents.dispatch(String(cx.params.agent ?? "worker"), String(cx.params.task ?? "")) };
  });

  app.onRequest("_zu/subagents/abort", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    if (!session.subagents) throw codedError(JSONRPC_CODE.methodNotFound, "subagents_unavailable", "no subagent surface");
    await session.subagents.abort(String(cx.params.runId ?? ""));
    return {};
  });

  // The interactive-tool mailbox (B2). `pending` is what the app pulls right
  // after session/open to re-render question cards across a restart;
  // `answer` resolves a live wait in the harness or, after the turn already
  // ended, injects the answer back into the conversation and triggers a turn.
  app.onRequest("_zu/ask/pending", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    return { asks: session.answers?.pending() ?? [] };
  });

  app.onRequest("_zu/ask/answer", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    if (!session.answers) return {};
    const toolCallId = String(cx.params.toolCallId ?? "");
    const { delivered } = await session.answers.answer(toolCallId, cx.params.response);
    if (delivered === "none") {
      throw codedError(JSONRPC_CODE.invalidParams, "unknown_ask", `no answerable ask for ${toolCallId}`);
    }
    return { delivered };
  });

  // The session plan (A2). The panel polls `list` and edits through `write`;
  // both hit the same store the `todo_*` tools use, so neither side needs a
  // push to stay current. A session without the surface (a fake, a bare
  // runtime) reads as an empty plan and refuses writes.
  app.onRequest("_zu/todos/list", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    return { enabled: session.todos?.enabled ?? false, plan: session.todos?.list() ?? EMPTY_PLAN };
  });

  app.onRequest("_zu/todos/write", (raw: any) => raw as any, async (cx) => {
    const session = requireSession(cx.params.sessionId);
    if (!session.todos) {
      throw codedError(JSONRPC_CODE.methodNotFound, "todos_unavailable", "no plan surface on this session");
    }
    const result = session.todos.write(cx.params.op);
    if (!result.ok) {
      throw codedError(JSONRPC_CODE.invalidParams, result.reason, TODO_FAILURE_TEXT[result.reason]);
    }
    return { plan: result.file };
  });

  app.onRequest("session/prompt", async (cx) => {
    const session = requireSession(cx.params.sessionId);
    if (busy.has(session.id)) {
      throw codedError(
        JSONRPC_CODE.invalidRequest,
        "session_busy",
        "a turn is already running; send session/cancel first",
        { sessionId: session.id },
      );
    }
    const blocks = cx.params.prompt as ContentBlock[];
    const converted = promptBlocksToText(blocks);
    if (!converted.ok) throw converted.error;
    busy.add(session.id);
    try {
      const stopReason = await session.prompt(converted.text);
      await pumps.get(session.id);
      return { stopReason };
    } catch (error) {
      throw normalizeError(error, "prompt_failed");
    } finally {
      busy.delete(session.id);
    }
  });

  app.onRequest("*", (cx) => {
    throw codedError(JSONRPC_CODE.methodNotFound, "method_not_found", `unknown method: ${cx.method}`);
  });

  app.onNotification("session/cancel", (cx) => {
    const session = sessions.get(cx.params.sessionId);
    if (!session) {
      log(`session/cancel for unknown session ${cx.params.sessionId}`);
      return;
    }
    void session.abort().catch((error) => {
      log(`abort failed for ${session.id}: ${error instanceof Error ? error.message : String(error)}`);
    });
  });

  function adopt(session: HarnessSession, client: AgentContext): void {
    const previous = sessions.get(session.id);
    if (previous && previous !== session) {
      try {
        previous.dispose();
      } catch (error) {
        log(`disposing replaced session ${session.id} failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    sessions.set(session.id, session);
    clients.set(session.id, client);
    session.attachClient(bridgeFor(client));
    pumps.delete(session.id);
    pump(session);
  }

  function bridgeFor(client: AgentContext): HarnessClientBridge {
    return {
      requestPermission: (params) => client.request("session/request_permission", params),
      elicit: (params) => client.request("elicitation/create", params),
      ask: (request) => client.request("_zu/ask/create", request as any) as Promise<AskResponse>,
    };
  }

  function pump(session: HarnessSession): void {
    pumps.set(session.id, Promise.resolve());
    session.onUpdate((update) => {
      const current = pumps.get(session.id) ?? Promise.resolve();
      const next = current.then(() => notifyUpdate(session.id, update));
      pumps.set(
        session.id,
        next.catch(() => {}),
      );
    });
  }

  async function announceCommands(session: HarnessSession): Promise<void> {
    await notifyUpdate(session.id, {
      sessionUpdate: "available_commands_update",
      availableCommands: session.availableCommands(),
    });
    // The banner follows the command table on new/load/resume; a session
    // without one (a fake, a bare runtime) sends nothing rather than an
    // empty banner (issue #20 §5.3).
    if (!session.banner) return;
    const banner = session.banner();
    if (!banner) return;
    await notifyUpdate(session.id, { sessionUpdate: "_zu/session_banner", banner } as unknown as SessionUpdate);
  }

  async function notifyUpdate(sessionId: string, update: SessionUpdate): Promise<void> {
    const client = clients.get(sessionId);
    if (!client) {
      log(`no client context for session ${sessionId}; update dropped (${describe(update)})`);
      return;
    }
    try {
      const notification: SessionNotification = { sessionId, update };
      await client.notify("session/update", notification);
    } catch (error) {
      log(`session/update failed for ${sessionId}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  function requireSession(sessionId: string): HarnessSession {
    const session = sessions.get(sessionId);
    if (!session) throw sessionNotFoundError(sessionId);
    return session;
  }

  async function withLog(stringCode: string, op: () => Promise<HarnessSession>): Promise<HarnessSession> {
    try {
      return await op();
    } catch (error) {
      const normalized = normalizeError(error, stringCode);
      log(`${stringCode}: ${normalized.message}`);
      throw normalized;
    }
  }

  return app;
}

function describe(update: SessionUpdate): string {
  return String((update as { sessionUpdate?: unknown }).sessionUpdate ?? "update");
}

/** The `session/set_config_option` value variants collapse to the id string. */
function configValue(params: any): string {
  const value = params?.value;
  if (typeof value === "string") return value;
  if (typeof value === "boolean") return value ? "true" : "false";
  if (value && typeof value === "object" && typeof value.value === "string") return value.value;
  throw codedError(JSONRPC_CODE.invalidParams, "invalid_config_value", "the config value is missing");
}

/**
 * The schema parses `mcpServers` as an array that defaults on null only, so a
 * request that omits the field would be refused before the handler runs.
 * The harness ignores MCP servers anyway (pi keeps its own configuration), so
 * an absent field means an empty list.
 */
function mcpServersLenient<T>(raw: unknown): T {
  const value = (raw ?? {}) as Record<string, unknown>;
  if (value.mcpServers === undefined || value.mcpServers === null) value.mcpServers = [];
  return value as T;
}

/** The client-minted session id a `session/new` `_meta` may carry (issue
 * #56). Anything else — absent, non-string — means "the harness mints". */
function clientIdOf(params: unknown): string | undefined {
  const meta = (params as { _meta?: { sessionId?: unknown } | null } | undefined)?._meta;
  return typeof meta?.sessionId === "string" && meta.sessionId.length > 0 ? meta.sessionId : undefined;
}
