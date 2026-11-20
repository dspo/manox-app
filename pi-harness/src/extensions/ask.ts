/**
 * A1: `ask_user_question` — the model's structured questions (issue #12).
 *
 * Clean room: built against the issue SPEC and pi's public extension
 * contract only. ThinkRail's `pi-extensions/ask-user-question` was consulted
 * for schema field names and test titles (Apache-2.0, d891e421), never for
 * implementation or copy.
 *
 * One call is one round: the tool carries 1–4 questions, each with 1–4
 * options (label + description, optional recommendation reason). The client
 * renders them as a card docked above the composer; the answers come back
 * as a structured envelope that this file renders into model-readable text.
 *
 * B2's mailbox (src/interactions.ts) owns the hard parts — waiting, late
 * answers, crash repair, supersede — so the tool only asks and waits:
 *
 * - `execute` announces into the session's `AnswerRegistry` via the ask
 *   bridge, then forwards the questions to the client as `_zu/ask/create`.
 *   The announce entry survives a crash; the registry waiter lets
 *   `_zu/ask/answer` and `supersedeAll` reach the tool even if the
 *   create-request never returns (the user typed instead).
 * - Every settled path routes through `AnswerRegistry.answer`, so a receipt
 *   (`zu.ask.answer`) is always written and re-seeding stays idempotent.
 * - No wall-clock timeout: the user may walk away. Session close / process
 *   death are repaired by B2's dangling-call fix (§20.2). The abort signal
 *   is deliberately ignored: a cancelled turn leaves the ask pending, and a
 *   later answer still lands via B2's late-answer inject.
 *
 * Reserved labels (`Other`, `Chat about this`, …) are authored by the card,
 * never by the model: `execute` rejects them fail-closed with a short error.
 */

import { Type } from "typebox";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { INTERACTIVE_TOOLS, isSupersedeResponse, PENDING_ACK_TEXT } from "../interactions";
import type { ZuExtension } from "./registry";

/** The tool pi registers; also the B2 interactive-registry key. */
export const ASK_TOOL_NAME = "ask_user_question";

// --- wire shapes (harness ↔ client) --------------------------------------

export interface AskOption {
  label: string;
  description: string;
  recommendedReason?: string;
}

export interface AskQuestion {
  question: string;
  header: string;
  multiSelect: boolean;
  options: AskOption[];
}

/** harness → client as `_zu/ask/create` params. */
export interface AskRequest {
  sessionId: string;
  toolCallId: string;
  questions: AskQuestion[];
}

export type AskAnswer =
  | { kind: "option"; labels: string[]; /** A typed note supplements the picks. */ text?: string }
  | { kind: "custom"; text: string }
  | { kind: "discuss"; notes: string; /** The options the user leans toward. */ labels?: string[] };

/** client → harness as the `_zu/ask/create` response. */
export type AskResponse =
  | { action: "accept"; answers: AskAnswer[] }
  | { action: "cancel" }
  | { action: "supersede" };

/** The ask transport a running session supplies to the extension. */
export interface AskBridge {
  ask(request: Omit<AskRequest, "sessionId">): Promise<unknown>;
}

// --- envelope (model-facing text; the wording is part of the SPEC) --------

export const ASK_CANCELLED_TEXT = "The user dismissed the question. Continue without an answer.";

const CONTINUE_CODA = "\nYou can now continue with the user's answers in mind.";
const NO_ANSWER_TEXT = "(no answer)";

function renderAnswer(answer: AskAnswer | undefined): string {
  if (!answer) return NO_ANSWER_TEXT;
  if (answer.kind === "option") {
    // A question the user left unanswered submits an empty option list; a
    // pick can also carry a typed note, which supplements (never replaces) it.
    if (answer.labels.length === 0) return answer.text?.trim() ? answer.text : NO_ANSWER_TEXT;
    const picked = answer.labels.join(", ");
    return answer.text?.trim() ? `${picked} — additionally: ${answer.text}` : picked;
  }
  if (answer.kind === "custom") return answer.text;
  const notes = answer.notes.trim();
  const leaning =
    answer.labels && answer.labels.length > 0 ? ` Leaning toward: ${answer.labels.join(", ")}.` : "";
  return `wants to discuss${notes ? `: ${notes}` : ""}.${leaning} Do not guess; ask again in a later round.`;
}

/** The one envelope both live answers and late answers use. */
export function askEnvelopeText(questions: AskQuestion[], answers: AskAnswer[]): string {
  const lines = questions.map((q, i) => `- ${q.question}: ${renderAnswer(answers[i])}`);
  return `User has answered your questions:\n${lines.join("\n")}${CONTINUE_CODA}`;
}

// --- result builders -------------------------------------------------------

function errorResult(text: string): { content: { type: "text"; text: string }[]; details: { kind: "error" }; isError: true } {
  return { content: [{ type: "text", text }], details: { kind: "error" }, isError: true };
}

function cancelledResult(): { content: { type: "text"; text: string }[]; details: { kind: "cancelled" } } {
  return { content: [{ type: "text", text: ASK_CANCELLED_TEXT }], details: { kind: "cancelled" } };
}

function acceptAnswers(response: unknown): AskAnswer[] | null {
  if (!response || typeof response !== "object") return null;
  if ((response as { action?: unknown }).action !== "accept") return null;
  const answers = (response as { answers?: unknown }).answers;
  return Array.isArray(answers) ? (answers as AskAnswer[]) : null;
}

/**
 * The tool's `AgentToolResult` for a settled ask. A malformed accept counts
 * as dismissed: the turn must continue, never wedge on a protocol bug.
 */
export function buildAskResult(
  questions: AskQuestion[],
  response: unknown,
): { content: { type: "text"; text: string }[]; details: Record<string, unknown>; terminate?: boolean } {
  if (isSupersedeResponse(response)) {
    // B2's sentinel: the user typed instead. Ack so pi can continue the
    // session file, and end the turn — the typed message owns it now.
    return { content: [{ type: "text", text: PENDING_ACK_TEXT }], details: { kind: "ack" }, terminate: true };
  }
  const answers = acceptAnswers(response);
  if (!answers || answers.length !== questions.length) return cancelledResult();
  return {
    content: [{ type: "text", text: askEnvelopeText(questions, answers) }],
    details: { kind: "answers", answers },
  };
}

/** Late-answer text for B2's inject (`describeResponse` of the registry). */
export function describeAskResponse(args: unknown, response: unknown): string {
  // Supersede never reaches here via the registry (it resolves waiters in
  // process); belt and braces anyway — and never echo PENDING_ACK_TEXT.
  if (isSupersedeResponse(response)) return ASK_CANCELLED_TEXT;
  const questions = questionsFromArgs(args);
  const answers = acceptAnswers(response);
  if (questions && answers && answers.length === questions.length) return askEnvelopeText(questions, answers);
  return ASK_CANCELLED_TEXT;
}

function questionsFromArgs(args: unknown): AskQuestion[] | null {
  if (!args || typeof args !== "object") return null;
  const raw = (args as { questions?: unknown }).questions;
  if (!Array.isArray(raw) || raw.length === 0) return null;
  const questions: AskQuestion[] = [];
  for (const q of raw) {
    if (!q || typeof q !== "object") return null;
    const text = (q as { question?: unknown }).question;
    if (typeof text !== "string" || text.length === 0) return null;
    questions.push({ question: text, header: "", multiSelect: false, options: [] });
  }
  return questions;
}

// --- reserved labels (the card owns them, issue §4.1) ----------------------

const RESERVED_LABELS = new Set([
  "other",
  "type something.",
  "chat about this",
  "discuss this...", // "…" and "..." normalize alike
  "none of these",
  "next →",
]);

function normalizeLabel(label: string): string {
  return label.trim().toLowerCase().replace(/…/g, "...");
}

export function findReservedLabel(
  questions: AskQuestion[],
): { question: number; option: number; label: string } | null {
  for (const [qi, q] of questions.entries()) {
    for (const [oi, option] of q.options.entries()) {
      if (RESERVED_LABELS.has(normalizeLabel(option.label))) {
        return { question: qi, option: oi, label: option.label.trim() };
      }
    }
  }
  return null;
}

// --- schema & tool ---------------------------------------------------------

const AskOptionSchema = Type.Object({
  label: Type.String({ description: "Button text, e.g. \"Yes, run the tests\".", minLength: 1, maxLength: 60 }),
  description: Type.String({ description: "What choosing this option means or does.", maxLength: 400 }),
  recommendedReason: Type.Optional(
    Type.String({ description: "When this option is recommended: why.", maxLength: 160 }),
  ),
});

const AskQuestionSchema = Type.Object({
  question: Type.String({
    description: "The question itself, e.g. \"Which database should we use?\".",
    minLength: 1,
    maxLength: 500,
  }),
  header: Type.String({ description: "Very short card title, e.g. \"Database\".", minLength: 1, maxLength: 16 }),
  multiSelect: Type.Optional(Type.Boolean({ description: "Allow choosing several options. Default false." })),
  options: Type.Array(AskOptionSchema, {
    description: "1–4 options; aim for 2–4. Do not author reserved labels like Other.",
    minItems: 1,
    maxItems: 4,
  }),
});

const AskParams = Type.Object({
  questions: Type.Array(AskQuestionSchema, {
    description: "1–4 questions, asked together on one card.",
    minItems: 1,
    maxItems: 4,
  }),
});

function normalizeQuestions(raw: unknown): AskQuestion[] {
  const list = Array.isArray(raw) ? raw : [];
  return list.map((item: any) => ({
    question: String(item?.question ?? ""),
    header: String(item?.header ?? ""),
    multiSelect: item?.multiSelect === true,
    options: (Array.isArray(item?.options) ? item.options : []).map((option: any) => ({
      label: String(option?.label ?? ""),
      description: String(option?.description ?? ""),
      ...(option?.recommendedReason === undefined ? {} : { recommendedReason: String(option.recommendedReason) }),
    })),
  }));
}

/** The extension body; `bridge` is the per-session ref pi-session installs. */
export function askExtension(bridge: { current: AskBridge | null }) {
  return (_piSettings: unknown): ((pi: ExtensionAPI) => void) => {
    return (pi: ExtensionAPI): void => {
      pi.registerTool({
        name: ASK_TOOL_NAME,
        label: "Ask the user",
        description:
          "Ask the user 1–4 structured questions and wait for the answers. Each question has a short header and 2–4 options, each with a one-line description and optionally why it is recommended. Prefer this over free-text questions whenever the choice is enumerable: which fix to apply, whether to add a dependency, how to scope a refactor. One call is one round — ask follow-ups in a later call. The user may pick option(s), type a free-form answer, ask to discuss instead, or dismiss the card; the result text tells you which happened. Never author the card's reserved labels (Other, Chat about this, and similar). This tool blocks until the user answers.",
        promptSnippet: "ask the user structured questions with options",
        parameters: AskParams,
        executionMode: "sequential",
        async execute(toolCallId: string, params: any, _signal: unknown, _onUpdate: unknown, _ctx: unknown) {
          if (bridge.current === null) {
            return errorResult("No UI is attached to this session; ask the question as plain text instead.");
          }
          const questions = normalizeQuestions(params?.questions);
          const reserved = findReservedLabel(questions);
          if (reserved) {
            return errorResult(
              `"${reserved.label}" (question ${reserved.question + 1}, option ${reserved.option + 1}) is reserved for the question card; pick another label.`,
            );
          }
          const response = await bridge.current.ask({ toolCallId, questions });
          return buildAskResult(questions, response);
        },
      });
    };
  };
}

/** Registry entry for B3's `ZU_EXTENSIONS`; the bridge ref is per session. */
export function askExtensionEntry(bridge: { current: AskBridge | null }): ZuExtension {
  return { name: "ask", factory: askExtension(bridge) };
}

// A1 lands the first interactive tool: from here on B2's dangling-call
// repair and pending-ask scan know about `ask_user_question`.
INTERACTIVE_TOOLS[ASK_TOOL_NAME] = { name: ASK_TOOL_NAME, describeResponse: describeAskResponse };
