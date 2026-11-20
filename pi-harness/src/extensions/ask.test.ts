/**
 * A1: `ask_user_question` — schema, envelope, reserved labels, and the
 * bridge contract. The tool itself only asks and waits; B2's mailbox
 * (interactions.test.ts) owns waiting/supersede/receipt behavior and is
 * exercised with this tool registered (module import side effect).
 *
 * Clean room: schema field names and test titles follow the issue SPEC.
 */

import { describe, expect, test } from "bun:test";
import {
  ASK_CANCELLED_TEXT,
  ASK_TOOL_NAME,
  askEnvelopeText,
  askExtension,
  buildAskResult,
  describeAskResponse,
  findReservedLabel,
  type AskAnswer,
  type AskBridge,
  type AskQuestion,
} from "./ask";
import { INTERACTIVE_TOOLS, PENDING_ACK_TEXT } from "../interactions";

// --- fixtures --------------------------------------------------------------

function questions(count = 1, multi = false): AskQuestion[] {
  const base: AskQuestion = {
    question: "Which database should we use?",
    header: "Database",
    multiSelect: multi,
    options: [
      { label: "Postgres", description: "Boring and proven", recommendedReason: "Team knows it" },
      { label: "SQLite", description: "Zero ops" },
    ],
  };
  return count === 1 ? [base] : [base, { ...base, question: "Second question?", header: "Second" }];
}

/** Captures asks and answers from a scripted queue. */
function scriptedBridge(responses: unknown[]): { bridge: { current: AskBridge | null }; asks: unknown[] } {
  const asks: unknown[] = [];
  let i = 0;
  return {
    asks,
    bridge: {
      current: {
        ask: (request) => {
          asks.push(request);
          return Promise.resolve(responses[i++]);
        },
      },
    },
  };
}

/** Registers the extension on a capture stub and returns the tool. */
function capture(bridge: { current: AskBridge | null }): any {
  let tool: any;
  const install = askExtension(bridge)(undefined);
  install({ registerTool: (t: any) => (tool = t) } as any);
  return tool;
}

// --- registration ----------------------------------------------------------

describe("ask_user_question registration", () => {
  test("registers under the B2 interactive registry", () => {
    expect(INTERACTIVE_TOOLS[ASK_TOOL_NAME]?.name).toBe(ASK_TOOL_NAME);
    expect(typeof INTERACTIVE_TOOLS[ASK_TOOL_NAME]?.describeResponse).toBe("function");
  });

  test("execute resolves to a sequential tool with the documented shape", () => {
    const tool = capture(scriptedBridge([]).bridge);
    expect(tool.name).toBe(ASK_TOOL_NAME);
    expect(tool.executionMode).toBe("sequential");
    expect(tool.parameters.properties.questions).toBeTruthy();
  });
});

// --- execute ---------------------------------------------------------------

describe("ask_user_question execute", () => {
  test("no UI attached → error telling the model to ask in plain text", async () => {
    const tool = capture({ current: null });
    const result = await tool.execute("tc1", { questions: questions() }, undefined, undefined, {});
    expect(result.isError).toBe(true);
    expect(result.content[0].text).toContain("UI is attached");
  });

  test("a live accept renders the answers envelope", async () => {
    const scripted = scriptedBridge([
      { action: "accept", answers: [{ kind: "option", labels: ["Postgres"] }] },
    ]);
    const tool = capture(scripted.bridge);
    const result = await tool.execute("tc1", { questions: questions() }, undefined, undefined, {});
    expect(result.isError).toBeUndefined();
    expect(result.content[0].text).toBe(
      askEnvelopeText(questions(), [{ kind: "option", labels: ["Postgres"] }]),
    );
    expect(result.details).toEqual({ kind: "answers", answers: [{ kind: "option", labels: ["Postgres"] }] });
    // the bridge saw the normalized questions (multiSelect filled in)
    expect((scripted.asks[0] as any).toolCallId).toBe("tc1");
    expect((scripted.asks[0] as any).questions[0].multiSelect).toBe(false);
  });

  test("multiSelect arrives as boolean on the wire", async () => {
    const scripted = scriptedBridge([{ action: "cancel" }]);
    const tool = capture(scripted.bridge);
    await tool.execute(
      "tc2",
      { questions: [{ ...questions()[0], multiSelect: true }] },
      undefined,
      undefined,
      {},
    );
    expect((scripted.asks[0] as any).questions[0].multiSelect).toBe(true);
  });

  test("dismissal renders the continue-without-answer text", async () => {
    const tool = capture(scriptedBridge([{ action: "cancel" }]).bridge);
    const result = await tool.execute("tc3", { questions: questions() }, undefined, undefined, {});
    expect(result.content[0].text).toBe(ASK_CANCELLED_TEXT);
    expect(result.details).toEqual({ kind: "cancelled" });
  });

  test("supersede renders the B2 ack and terminates the turn", async () => {
    const tool = capture(scriptedBridge([{ action: "supersede" }]).bridge);
    const result = await tool.execute("tc4", { questions: questions() }, undefined, undefined, {});
    expect(result.content[0].text).toBe(PENDING_ACK_TEXT);
    expect(result.details).toEqual({ kind: "ack" });
    expect(result.terminate).toBe(true);
  });

  test("a malformed accept counts as dismissed, never wedges the turn", async () => {
    const tool = capture(scriptedBridge([{ action: "accept", answers: [] }]).bridge);
    const result = await tool.execute("tc5", { questions: questions() }, undefined, undefined, {});
    expect(result.content[0].text).toBe(ASK_CANCELLED_TEXT);
  });
});

// --- reserved labels ---------------------------------------------------------

describe("reserved labels", () => {
  test("the issue's reserved set is rejected", () => {
    for (const label of ["Other", "Type something.", "Chat about this", "Discuss this…", "None of these", "Next →"]) {
      const probe = questions();
      probe[0].options[0].label = label;
      expect(findReservedLabel(probe)?.label).toBe(label);
    }
  });

  test("trim, case, and ellipsis variants are caught", () => {
    const probe = questions();
    probe[0].options[1].label = "  discuss this...  ";
    expect(findReservedLabel(probe)?.option).toBe(1);
  });

  test("execute refuses a reserved label without calling the bridge", async () => {
    const scripted = scriptedBridge([{ action: "cancel" }]);
    const tool = capture(scripted.bridge);
    const probe = questions();
    probe[0].options[0].label = "Other";
    const result = await tool.execute("tc6", { questions: probe }, undefined, undefined, {});
    expect(result.isError).toBe(true);
    expect(result.content[0].text).toContain("reserved");
    expect(scripted.asks).toHaveLength(0);
  });

  test("model-authored labels pass", () => {
    const probe = questions();
    probe[0].options[0].label = "otherwise";
    expect(findReservedLabel(probe)).toBeNull();
  });
});

// --- envelope -----------------------------------------------------------------

describe("envelope", () => {
  test("one line per question, options joined, coda appended", () => {
    const answers: AskAnswer[] = [
      { kind: "option", labels: ["Postgres", "SQLite"] },
      { kind: "custom", text: "whichever is faster" },
    ];
    const text = askEnvelopeText(questions(2, true), answers);
    expect(text).toContain("- Which database should we use?: Postgres, SQLite");
    expect(text).toContain("- Second question?: whichever is faster");
    expect(text.endsWith("You can now continue with the user's answers in mind.")).toBe(true);
  });

  test("an unanswered question reads (no answer)", () => {
    // Every question is optional: the card sends an empty option list, which
    // must read like a missing answer (never an empty line after the colon).
    const text = askEnvelopeText(questions(2), [
      { kind: "option", labels: [] },
      { kind: "option", labels: ["SQLite"] },
    ]);
    expect(text).toContain("- Which database should we use?: (no answer)");
    expect(text).toContain("- Second question?: SQLite");
  });

  test("a pick with a typed note keeps both", () => {
    // The note supplements the choice; it must never replace the labels.
    const text = askEnvelopeText(questions(), [
      { kind: "option", labels: ["Postgres"], text: "but check with the team first" },
    ]);
    expect(text).toContain(
      "- Which database should we use?: Postgres — additionally: but check with the team first",
    );
  });

  test("discuss answers carry the no-guess instruction", () => {
    const text = askEnvelopeText(questions(), [{ kind: "discuss", notes: "pricing first" }]);
    expect(text).toContain("wants to discuss: pricing first. Do not guess; ask again in a later round.");
  });

  test("discuss without notes still tells the model not to guess", () => {
    // The card's toggle is a mode, so an empty notes field is still a
    // discussion (the model must ask again, not invent an answer).
    const text = askEnvelopeText(questions(), [{ kind: "discuss", notes: "" }]);
    expect(text).toContain(
      "wants to discuss. Do not guess; ask again in a later round.",
    );
  });

  test("discuss keeps the leaning picks", () => {
    const text = askEnvelopeText(questions(), [
      { kind: "discuss", notes: "pricing first", labels: ["SQLite"] },
    ]);
    expect(text).toContain(
      "wants to discuss: pricing first. Leaning toward: SQLite. Do not guess; ask again in a later round.",
    );
  });

  test("describeAnswer for late injects uses the same envelope", () => {
    const args = { questions: questions() };
    const response = { action: "accept", answers: [{ kind: "option", labels: ["SQLite"] }] };
    expect(describeAskResponse(args, response)).toBe(
      askEnvelopeText(questions(), [{ kind: "option", labels: ["SQLite"] }]),
    );
  });

  test("describeAnswer never echoes the pending-ack sentinel", () => {
    expect(describeAskResponse({ questions: questions() }, { action: "supersede" })).not.toContain(PENDING_ACK_TEXT);
    expect(describeAskResponse(null, { action: "cancel" })).toBe(ASK_CANCELLED_TEXT);
    expect(describeAskResponse({ questions: questions() }, undefined)).toBe(ASK_CANCELLED_TEXT);
  });
});
