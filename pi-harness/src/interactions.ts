/**
 * Interactive-tool mailbox (B2): pending acks, restart repair, late answers.
 *
 * Interactive tools (A1's `ask_user_question` is the first) may not be able to
 * answer immediately. Instead of blocking a turn until the user types, the
 * tool's call is *parked*: the turn ends with a pending-ack toolResult
 * (`terminate: true`), the ask is announced in the session file, and the
 * answer arrives later — either resolving the live waiter or, after a
 * restart, being injected back into the conversation as its own message.
 *
 * Clean room (Apache-2.0 upstream ThinkRail d891e421): only the issue SPEC and
 * public pi contracts were read; no upstream implementation code.
 *
 * Fixture-verified marker strategy (pi 1.5.0): a toolResult's `details` IS
 * persisted into the session file and re-read via `buildSessionProjection()`
 * after a reopen. So the primary pending marker is
 * `details.kind === "ack"` (PENDING_ACK_DETAILS); a content text equality
 * fallback (PENDING_ACK_TEXT, exact single block) covers acks written by a
 * writer that dropped details. A1 must not echo this text in
 * `describeResponse` — it is reserved as the marker.
 */

import { SessionManager, type SessionEntry } from "@earendil-works/pi-coding-agent";

/** The message fields the mailbox reads, structurally: pi's exported message
 * union (reached through `SessionEntry`) now also carries branch-summary
 * shapes these helpers never touch. */
interface MailboxMessage {
  role?: string;
  content?: unknown;
  toolCallId?: unknown;
  toolName?: unknown;
}

/** Model-readable ack the parked tool call resolves with when unanswered. */
export const PENDING_ACK_TEXT =
  "The user has not answered yet. Continue without it; the answer will arrive as its own message.";

/** Persisted on the ack toolResult; primary restart marker (see header). */
export const PENDING_ACK_DETAILS = { kind: "ack" } as const;

export interface InteractiveTool {
  /** The name the tool registers under (`pi.registerTool` name). */
  name: string;
  /** Restart-side: turn a persisted answer into model-readable text. */
  describeResponse(args: unknown, response: unknown): string;
}

/**
 * Registry of interactive tool shapes, keyed by tool name. A1 provides
 * `ask_user_question`; until that lands this stays empty — every repair and
 * pending scan below is a no-op against an empty registry (hence the fixture
 * tests).
 */
export const INTERACTIVE_TOOLS: Record<string, InteractiveTool> = {};

/** Custom entry the asking tool writes just before it starts waiting. */
export const ASK_ANNOUNCE_TYPE = "zu.ask";
/** Custom entry written once a late answer is delivered (idempotency receipt). */
export const ASK_ANSWER_TYPE = "zu.ask.answer";
/** Custom message a late answer injects into the conversation (display:true). */
export const TOOL_RESPONSE_TYPE = "zu.tool-response";
/** What a superseded wait resolves with; the tool answers with the ack. */
export const SUPERSEDE_RESPONSE = { action: "supersede" } as const;

/** True for the supersede sentinel, by value: across the wire it arrives as a
 * freshly parsed object, so reference equality would miss it. The value is
 * reserved — real ask answers (option/custom/discuss) carry no `action`. */
export function isSupersedeResponse(response: unknown): boolean {
  return isRecord(response) && response.action === "supersede";
}

interface ToolCallRef {
  id: string;
  name: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function messageOf(entry: SessionEntry): MailboxMessage | undefined {
  return entry.type === "message" ? entry.message : undefined;
}

function toolCallsOf(message: MailboxMessage | undefined): ToolCallRef[] {
  if (!isRecord(message) || !Array.isArray(message.content)) return [];
  const calls: ToolCallRef[] = [];
  for (const block of message.content) {
    if (
      isRecord(block) &&
      block.type === "toolCall" &&
      typeof block.id === "string" &&
      typeof block.name === "string"
    ) {
      calls.push({ id: block.id, name: block.name });
    }
  }
  return calls;
}

/**
 * The session file's active branch, trimmed to the active tail: everything
 * after the last compaction/branch-summary entry. Repairs and pending scans
 * only ever look here — a parked call that has already been summarized away
 * is no longer in the model's transcript, and appending a toolResult after a
 * compaction entry would produce an orphan result without its call.
 */
function branchTail(manager: SessionManager): { branch: SessionEntry[]; tail: SessionEntry[] } {
  const branch = manager.getBranch();
  let tailStart = 0;
  for (let index = 0; index < branch.length; index++) {
    const type = branch[index]!.type;
    if (type === "compaction" || type === "branch_summary") tailStart = index + 1;
  }
  return { branch, tail: branch.slice(tailStart) };
}

/** True for a parked toolResult: ack details, or the reserved ack text. */
export function isPendingAck(message: unknown): boolean {
  if (!isRecord(message) || message.role !== "toolResult") return false;
  if (isRecord(message.details) && message.details.kind === "ack") return true;
  // Fallback marker: exactly the reserved text, one text block. The primary
  // `details.kind` marker is fixture-verified to survive restarts (header).
  const content = message.content;
  return (
    Array.isArray(content) &&
    content.length === 1 &&
    isRecord(content[0]) &&
    content[0].type === "text" &&
    content[0].text === PENDING_ACK_TEXT
  );
}

/**
 * Restart repair: the active tail's last assistant entry may hold parked
 * interactive tool calls with no toolResult (the process died while waiting).
 * Append a pending-ack toolResult for each so the next turn is a valid
 * transcript. Idempotent by construction — a call that already has any
 * toolResult is left alone, and non-interactive calls are never repaired
 * (their absence means something else; do not guess).
 *
 * Returns the number of appended acks.
 */
export function repairDanglingInteractiveCalls(
  manager: SessionManager,
  interactive: Record<string, InteractiveTool>,
): number {
  const { tail } = branchTail(manager);
  let assistantIndex = -1;
  for (let index = tail.length - 1; index >= 0; index--) {
    if (messageOf(tail[index]!)?.role === "assistant") {
      assistantIndex = index;
      break;
    }
  }
  if (assistantIndex < 0) return 0;
  const calls = toolCallsOf(messageOf(tail[assistantIndex]!)).filter((call) => Boolean(interactive[call.name]));
  if (calls.length === 0) return 0;
  const answered = new Set<string>();
  for (let index = assistantIndex + 1; index < tail.length; index++) {
    const message = messageOf(tail[index]!);
    if (message?.role === "toolResult" && typeof message.toolCallId === "string") {
      answered.add(message.toolCallId);
    }
  }
  let repaired = 0;
  for (const call of calls) {
    if (answered.has(call.id)) continue;
    manager.appendMessage({
      role: "toolResult",
      toolCallId: call.id,
      toolName: call.name,
      content: [{ type: "text", text: PENDING_ACK_TEXT }],
      isError: false,
      details: PENDING_ACK_DETAILS,
      timestamp: Date.now(),
    } as Parameters<SessionManager["appendMessage"]>[0]);
    repaired++;
  }
  return repaired;
}

export interface PendingAskRecord {
  toolCallId: string;
  args: unknown;
}

/**
 * Restart-side pending asks: parked toolResults in the active tail that are
 * answered with an ack, announced with args, and not yet answered. An ack
 * without a `zu.ask` announce entry is not listed — without args the question
 * cannot be rendered (and `describeResponse` would have nothing to describe).
 */
export function pendingAsksOf(
  manager: SessionManager,
  interactive: Record<string, InteractiveTool>,
): PendingAskRecord[] {
  const { branch, tail } = branchTail(manager);
  const argsById = new Map<string, unknown>();
  const answered = new Set<string>();
  for (const entry of branch) {
    if (entry.type !== "custom") continue;
    if (!isRecord(entry.data) || typeof entry.data.toolCallId !== "string") continue;
    if (entry.customType === ASK_ANNOUNCE_TYPE) argsById.set(entry.data.toolCallId, entry.data.args);
    if (entry.customType === ASK_ANSWER_TYPE) answered.add(entry.data.toolCallId);
  }
  const asks: PendingAskRecord[] = [];
  for (const entry of tail) {
    const message = messageOf(entry);
    if (message?.role !== "toolResult" || !isPendingAck(message)) continue;
    const toolCallId = typeof message.toolCallId === "string" ? message.toolCallId : "";
    const toolName = typeof message.toolName === "string" ? message.toolName : "";
    if (!toolCallId || !interactive[toolName] || answered.has(toolCallId)) continue;
    const args = argsById.get(toolCallId);
    if (args === undefined) continue;
    asks.push({ toolCallId, args });
  }
  return asks;
}

export type AnswerDelivery = "live" | "late" | "none";

/**
 * Session-level answer registry. Live waits resolve in-process; answers that
 * arrive after a restart (or for a turn that already ended) are injected back
 * into the conversation via `inject`. Every delivered answer — live,
 * superseded or late — leaves a `zu.ask.answer` receipt entry: the receipt is
 * what retires an ask on restart (`pendingAsksOf` trusts nothing else) and
 * what makes a repeated delivery a no-op.
 */
export class AnswerRegistry {
  private readonly waiting = new Map<string, (response: unknown) => void>();

  constructor(
    private readonly manager: SessionManager,
    private readonly interactive: Record<string, InteractiveTool>,
    private readonly hooks: {
      /** Injects a late answer as its own conversation message and triggers a turn. */
      inject(text: string, meta: { toolCallId: string; response: unknown }): Promise<void>;
      log(line: string): void;
    },
  ) {}

  /**
   * The asking tool's side: persist the announce entry (args survive the
   * restart for late answers) and record the live waiter.
   */
  announce(toolCallId: string, args: unknown, resolve: (response: unknown) => void): void {
    this.manager.appendCustomEntry(ASK_ANNOUNCE_TYPE, { toolCallId, args });
    this.waiting.set(toolCallId, resolve);
  }

  /** App-side delivery for one parked call. */
  async answer(toolCallId: string, response: unknown): Promise<{ delivered: AnswerDelivery }> {
    const resolve = this.waiting.get(toolCallId);
    if (resolve) {
      this.waiting.delete(toolCallId);
      // The receipt — not the ack toolResult — is what retires the ask on
      // restart, so a live answer writes one too: without it the next open
      // would re-seed an answered question as a card, and answering the card
      // would late-inject the answer a second time.
      this.manager.appendCustomEntry(ASK_ANSWER_TYPE, { toolCallId, response });
      resolve(response);
      return { delivered: "live" };
    }
    if (this.hasReceipt(toolCallId)) return { delivered: "late" };
    // A seeded card (no live waiter after a restart) the user skipped by
    // typing arrives as the sentinel: retire it with a receipt only. The
    // sentinel is not an answer — it must never reach describeResponse and
    // be injected into the conversation as one.
    if (isSupersedeResponse(response)) {
      if (!this.pending().some((ask) => ask.toolCallId === toolCallId)) return { delivered: "none" };
      this.manager.appendCustomEntry(ASK_ANSWER_TYPE, { toolCallId, response });
      this.hooks.log(`superseded parked ask ${toolCallId} in ${this.manager.getSessionId()}`);
      return { delivered: "late" };
    }
    const ask = this.lateAsk(toolCallId);
    if (!ask) return { delivered: "none" };
    const text = ask.tool.describeResponse(ask.args, response);
    await this.hooks.inject(text, { toolCallId, response });
    this.manager.appendCustomEntry(ASK_ANSWER_TYPE, { toolCallId, response });
    this.hooks.log(`late answer injected for ${toolCallId} in ${this.manager.getSessionId()}`);
    return { delivered: "late" };
  }

  /** Restart-side list for the app's question cards. */
  pending(): PendingAskRecord[] {
    return pendingAsksOf(this.manager, this.interactive);
  }

  /**
   * The user typed their own next message: every live wait resolves with the
   * supersede sentinel, and the tool answers with the pending ack. Each
   * superseded ask also gets a receipt — the ack alone does not retire it
   * (`pendingAsksOf` trusts receipts only), and without one the next open
   * would re-seed a question the user already skipped. The announce entries
   * stay: history is not rewritten.
   */
  supersedeAll(): void {
    const waiting = [...this.waiting.entries()];
    this.waiting.clear();
    for (const [toolCallId, resolve] of waiting) {
      this.manager.appendCustomEntry(ASK_ANSWER_TYPE, { toolCallId, response: SUPERSEDE_RESPONSE });
      resolve(SUPERSEDE_RESPONSE);
    }
  }

  private hasReceipt(toolCallId: string): boolean {
    return this.manager
      .getBranch()
      .some(
        (entry) =>
          entry.type === "custom" &&
          entry.customType === ASK_ANSWER_TYPE &&
          isRecord(entry.data) &&
          entry.data.toolCallId === toolCallId,
      );
  }

  /** The announce entry + owning tool for a late answer, or null if unknown. */
  private lateAsk(toolCallId: string): { tool: InteractiveTool; args: unknown } | null {
    const branch = this.manager.getBranch();
    let name = "";
    for (const entry of branch) {
      for (const call of toolCallsOf(messageOf(entry))) {
        if (call.id === toolCallId) name = call.name;
      }
    }
    const tool = this.interactive[name];
    if (!tool) return null;
    let args: unknown;
    for (const entry of branch) {
      if (
        entry.type === "custom" &&
        entry.customType === ASK_ANNOUNCE_TYPE &&
        isRecord(entry.data) &&
        entry.data.toolCallId === toolCallId
      ) {
        args = entry.data.args;
      }
    }
    if (args === undefined) return null;
    return { tool, args };
  }
}
