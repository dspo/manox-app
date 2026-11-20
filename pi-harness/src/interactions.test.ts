/**
 * Interactive-tool mailbox (B2): restart repair, pending asks, the answer
 * registry. Real SessionManager instances over temp session files — no pi
 * agent, no network. The fixture tool stands in for A1's
 * `ask_user_question` (INTERACTIVE_TOOLS is deliberately empty in B2).
 */

import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SessionManager, type AgentMessage } from "@earendil-works/pi-coding-agent";
import {
  ASK_ANNOUNCE_TYPE,
  ASK_ANSWER_TYPE,
  AnswerRegistry,
  PENDING_ACK_DETAILS,
  PENDING_ACK_TEXT,
  SUPERSEDE_RESPONSE,
  isPendingAck,
  pendingAsksOf,
  repairDanglingInteractiveCalls,
  type InteractiveTool,
} from "./interactions";

const roots: string[] = [];

function makeManager(): SessionManager {
  const root = mkdtempSync(join(tmpdir(), "zu-interactions-"));
  roots.push(root);
  return SessionManager.create(join(root, "proj"), join(root, "sessions"));
}

afterEach(() => {
  while (roots.length > 0) {
    rmSync(roots.pop()!, { recursive: true, force: true });
  }
});

const FIXTURE_TOOL: InteractiveTool = {
  name: "ask_user_question",
  describeResponse: (_args, response) => `The user later answered: ${JSON.stringify(response)}`,
};
const FIXTURE_TOOLS: Record<string, InteractiveTool> = { ask_user_question: FIXTURE_TOOL };

function appendUser(manager: SessionManager, text: string): void {
  manager.appendMessage({
    role: "user",
    content: [{ type: "text", text }],
    timestamp: Date.now(),
  } as AgentMessage);
}

function appendAssistantCalls(manager: SessionManager, calls: Array<{ id: string; name: string }>): void {
  manager.appendMessage({
    role: "assistant",
    content: calls.map((call) => ({ type: "toolCall", id: call.id, name: call.name, arguments: {} })),
    timestamp: Date.now(),
  } as AgentMessage);
}

function appendResult(
  manager: SessionManager,
  toolCallId: string,
  toolName: string,
  details?: unknown,
): void {
  manager.appendMessage({
    role: "toolResult",
    toolCallId,
    toolName,
    content: [{ type: "text", text: "done" }],
    isError: false,
    ...(details === undefined ? {} : { details }),
    timestamp: Date.now(),
  } as AgentMessage);
}

function tailMessages(manager: SessionManager): AgentMessage[] {
  return manager
    .getBranch()
    .filter((entry) => entry.type === "message")
    .map((entry) => (entry.message as AgentMessage));
}

describe("repairDanglingInteractiveCalls", () => {
  test("appends one ack for a dangling interactive call, idempotently", () => {
    const manager = makeManager();
    appendUser(manager, "which database?");
    appendAssistantCalls(manager, [{ id: "tc1", name: "ask_user_question" }]);
    expect(repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS)).toBe(1);
    const results = tailMessages(manager).filter((message) => message.role === "toolResult");
    expect(results.length).toBe(1);
    const ack = results[0] as Record<string, unknown>;
    expect(ack.toolCallId).toBe("tc1");
    expect(JSON.stringify(ack.content)).toContain(PENDING_ACK_TEXT);
    expect(ack.details).toEqual(PENDING_ACK_DETAILS);
    expect(isPendingAck(ack)).toBe(true);
    // A second pass finds the ack already in place.
    expect(repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS)).toBe(0);
    // And the marker survives a reopen via pi's own projection.
    const reopened = SessionManager.open(manager.getSessionFile(), join(manager.getSessionFile(), ".."));
    const projected = reopened
      .buildSessionProjection()
      .messages.filter((message) => (message as Record<string, unknown>).role === "toolResult");
    expect((projected[0] as Record<string, unknown>).details).toEqual(PENDING_ACK_DETAILS);
  });

  test("leaves non-interactive calls and answered calls alone", () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc2", name: "read" }]);
    appendAssistantCalls(manager, [{ id: "tc3", name: "ask_user_question" }]);
    appendResult(manager, "tc3", "ask_user_question");
    expect(repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS)).toBe(0);
    expect(tailMessages(manager).filter((message) => message.role === "toolResult").length).toBe(1);
  });

  test("fail-closed: a gap that is not the active tail's last turn stays broken", () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc4", name: "ask_user_question" }]);
    // A later assistant entry means the turn moved on without the answer;
    // appending an ack now would falsify history.
    appendAssistantCalls(manager, [{ id: "tc5", name: "read" }]);
    expect(repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS)).toBe(0);
  });

  test("a mixed batch repairs only the interactive call", () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [
      { id: "tc6", name: "ask_user_question" },
      { id: "tc7", name: "read" },
    ]);
    appendResult(manager, "tc7", "read");
    expect(repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS)).toBe(1);
    const ids = tailMessages(manager)
      .filter((message) => message.role === "toolResult")
      .map((message) => (message as Record<string, unknown>).toolCallId);
    expect(ids).toEqual(["tc7", "tc6"]);
  });

  test("does not reach across a compaction boundary", () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc8", name: "ask_user_question" }]);
    manager.appendCompaction("summary of the ask", null, 100);
    expect(repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS)).toBe(0);
  });
});

describe("pendingAsksOf", () => {
  test("lists acked announced asks; skips unannounced and answered ones", () => {
    const manager = makeManager();
    appendUser(manager, "plan?");
    appendAssistantCalls(manager, [
      { id: "tc9", name: "ask_user_question" },
      { id: "tc10", name: "ask_user_question" },
    ]);
    manager.appendCustomEntry(ASK_ANNOUNCE_TYPE, { toolCallId: "tc9", args: { question: "which?" } });
    // Nothing is acked yet — and the announce alone does not make a card.
    expect(pendingAsksOf(manager, FIXTURE_TOOLS)).toEqual([]);
    repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS);
    const asks = pendingAsksOf(manager, FIXTURE_TOOLS);
    expect(asks.length).toBe(1);
    expect(asks[0]).toEqual({ toolCallId: "tc9", args: { question: "which?" } });
    // The receipt retires the card; the unannounced tc10 never surfaces.
    manager.appendCustomEntry(ASK_ANSWER_TYPE, { toolCallId: "tc9", response: { kind: "custom", text: "sqlite" } });
    expect(pendingAsksOf(manager, FIXTURE_TOOLS)).toEqual([]);
  });

  test("an empty registry lists nothing", () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc11", name: "ask_user_question" }]);
    manager.appendCustomEntry(ASK_ANNOUNCE_TYPE, { toolCallId: "tc11", args: {} });
    repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS);
    expect(pendingAsksOf(manager, {})).toEqual([]);
  });
});

describe("AnswerRegistry", () => {
  test("answer resolves the live waiter and its receipt retires the ask", async () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc12", name: "ask_user_question" }]);
    const registry = new AnswerRegistry(manager, FIXTURE_TOOLS, {
      inject: async () => {
        throw new Error("live answers must not inject");
      },
      log: () => {},
    });
    const delivered: unknown[] = [];
    registry.announce("tc12", { question: "which?" }, (response) => delivered.push(response));
    // The parked shape: the turn ended with the ack toolResult, so the ask
    // is pending — and would re-seed as a card after a restart.
    repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS);
    expect(registry.pending().map((ask) => ask.toolCallId)).toEqual(["tc12"]);
    const reply = await registry.answer("tc12", { kind: "option", labels: ["postgres"] });
    expect(reply).toEqual({ delivered: "live" });
    expect(delivered).toEqual([{ kind: "option", labels: ["postgres"] }]);
    // The receipt — not the ack — is what retires the ask; without it the
    // next open would re-seed an answered question and answering the card
    // would late-inject the answer a second time.
    const receipts = manager
      .getBranch()
      .filter((entry) => entry.type === "custom" && entry.customType === ASK_ANSWER_TYPE);
    expect(receipts.length).toBe(1);
    expect(receipts[0]!.data).toEqual({ toolCallId: "tc12", response: { kind: "option", labels: ["postgres"] } });
    expect(registry.pending()).toEqual([]);
    const reopened = SessionManager.open(manager.getSessionFile(), join(manager.getSessionFile(), ".."));
    expect(pendingAsksOf(reopened, FIXTURE_TOOLS)).toEqual([]);
  });

  test("a late answer injects once and writes one receipt", async () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc13", name: "ask_user_question" }]);
    manager.appendCustomEntry(ASK_ANNOUNCE_TYPE, { toolCallId: "tc13", args: { question: "which?" } });
    repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS);
    const injected: Array<{ text: string; meta: unknown }> = [];
    const registry = new AnswerRegistry(manager, FIXTURE_TOOLS, {
      inject: async (text, meta) => {
        injected.push({ text, meta });
      },
      log: () => {},
    });
    const first = await registry.answer("tc13", { kind: "option", labels: ["sqlite"] });
    expect(first).toEqual({ delivered: "late" });
    expect(injected.length).toBe(1);
    expect(injected[0]!.text).toBe('The user later answered: {"kind":"option","labels":["sqlite"]}');
    expect(injected[0]!.meta).toEqual({ toolCallId: "tc13", response: { kind: "option", labels: ["sqlite"] } });
    const receipts = manager
      .getBranch()
      .filter((entry) => entry.type === "custom" && entry.customType === ASK_ANSWER_TYPE);
    expect(receipts.length).toBe(1);
    // A repeated delivery is a receipt hit, not a second injection.
    const second = await registry.answer("tc13", { kind: "option", labels: ["sqlite"] });
    expect(second).toEqual({ delivered: "late" });
    expect(injected.length).toBe(1);
    expect(manager.getBranch().filter((entry) => entry.type === "custom" && entry.customType === ASK_ANSWER_TYPE).length).toBe(1);
  });

  test("a wire supersede retires a seeded ask without injecting it", async () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [{ id: "tc17", name: "ask_user_question" }]);
    manager.appendCustomEntry(ASK_ANNOUNCE_TYPE, { toolCallId: "tc17", args: { question: "which?" } });
    repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS);
    const injected: string[] = [];
    const registry = new AnswerRegistry(manager, FIXTURE_TOOLS, {
      inject: async (text) => {
        injected.push(text);
      },
      log: () => {},
    });
    expect(registry.pending().map((ask) => ask.toolCallId)).toEqual(["tc17"]);
    // Across the wire the sentinel is a freshly parsed object — the match
    // must be by value, not reference.
    const reply = await registry.answer("tc17", { action: "supersede" });
    expect(reply).toEqual({ delivered: "late" });
    expect(injected).toEqual([]);
    const receipts = manager
      .getBranch()
      .filter((entry) => entry.type === "custom" && entry.customType === ASK_ANSWER_TYPE);
    expect(receipts.length).toBe(1);
    expect(receipts[0]!.data).toEqual({ toolCallId: "tc17", response: { action: "supersede" } });
    expect(registry.pending()).toEqual([]);
    const reopened = SessionManager.open(manager.getSessionFile(), join(manager.getSessionFile(), ".."));
    expect(pendingAsksOf(reopened, FIXTURE_TOOLS)).toEqual([]);
    // A second sentinel is a receipt hit, not a second receipt.
    expect(await registry.answer("tc17", SUPERSEDE_RESPONSE)).toEqual({ delivered: "late" });
    expect(manager.getBranch().filter((entry) => entry.type === "custom" && entry.customType === ASK_ANSWER_TYPE).length).toBe(1);
    // A sentinel for an unknown ask stays an error, not a stray receipt.
    expect(await registry.answer("tc-absent", SUPERSEDE_RESPONSE)).toEqual({ delivered: "none" });
  });

  test("an unknown tool call id answers none", async () => {
    const manager = makeManager();
    const registry = new AnswerRegistry(manager, FIXTURE_TOOLS, {
      inject: async () => {
        throw new Error("must not inject");
      },
      log: () => {},
    });
    expect(await registry.answer("tc-absent", {})).toEqual({ delivered: "none" });
  });

  test("supersedeAll resolves every waiter with the sentinel and retires the asks", async () => {
    const manager = makeManager();
    appendUser(manager, "go");
    appendAssistantCalls(manager, [
      { id: "tc14", name: "ask_user_question" },
      { id: "tc15", name: "ask_user_question" },
    ]);
    const registry = new AnswerRegistry(manager, FIXTURE_TOOLS, { inject: async () => {}, log: () => {} });
    const resolved: unknown[] = [];
    registry.announce("tc14", { question: "a?" }, (response) => resolved.push(response));
    registry.announce("tc15", { question: "b?" }, (response) => resolved.push(response));
    // Parked: both asks would re-seed as cards after a restart.
    repairDanglingInteractiveCalls(manager, FIXTURE_TOOLS);
    expect(registry.pending().length).toBe(2);
    registry.supersedeAll();
    expect(resolved).toEqual([SUPERSEDE_RESPONSE, SUPERSEDE_RESPONSE]);
    // Each skipped ask leaves a receipt: the ack alone does not retire it,
    // and without receipts the next open would re-seed questions the user
    // already answered themselves.
    const receipts = manager
      .getBranch()
      .filter((entry) => entry.type === "custom" && entry.customType === ASK_ANSWER_TYPE);
    expect(receipts.length).toBe(2);
    expect(registry.pending()).toEqual([]);
    const reopened = SessionManager.open(manager.getSessionFile(), join(manager.getSessionFile(), ".."));
    expect(pendingAsksOf(reopened, FIXTURE_TOOLS)).toEqual([]);
    // Superseding again is a no-op; a fresh wait still works.
    const fresh: unknown[] = [];
    registry.announce("tc16", {}, (response) => fresh.push(response));
    registry.supersedeAll();
    expect(fresh).toEqual([SUPERSEDE_RESPONSE]);
  });
});
