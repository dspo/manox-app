/**
 * Unit tests for the byte-stable prompt-section mechanism. Fake ExtensionAPI,
 * no pi, no session: the handler's contract is everything.
 */

import { describe, expect, test } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { DISPATCH_RULE, promptSectionExtension } from "./prompt-section";

// ---------------------------------------------------------------- fixtures

/** Fake ExtensionAPI that records `on` registrations per event name. */
function fakePi() {
  const registered = new Map<string, Array<(...args: unknown[]) => unknown>>();
  const pi = {
    on(event: string, handler: (...args: unknown[]) => unknown) {
      const list = registered.get(event) ?? [];
      list.push(handler);
      registered.set(event, list);
      return () => {
        const rest = (registered.get(event) ?? []).filter((h) => h !== handler);
        registered.set(event, rest);
      };
    },
  };
  return { pi: pi as unknown as ExtensionAPI, registered };
}

/** A minimal before_agent_start event: only the fields the handler may touch. */
function event(sections: Record<string, string> = {}): Record<string, unknown> {
  return { systemPromptOptions: { sections } };
}

// ------------------------------------------------------------------ tests

describe("promptSectionExtension", () => {
  test("registers exactly one before_agent_start handler and nothing else", () => {
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", DISPATCH_RULE)(pi);
    expect([...registered.keys()]).toEqual(["before_agent_start"]);
    expect(registered.get("before_agent_start")).toHaveLength(1);
  });

  test("writes the rule into its named section and returns undefined", () => {
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", DISPATCH_RULE)(pi);
    const handler = registered.get("before_agent_start")![0];
    const sections: Record<string, string> = {};
    const result = (handler as (e: unknown) => unknown)(event(sections));
    expect(sections["zu-dispatch"]).toBe(DISPATCH_RULE);
    expect(result).toBeUndefined();
  });

  test("is byte-stable: two consecutive runs write the identical string", () => {
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", DISPATCH_RULE)(pi);
    const handler = registered.get("before_agent_start")![0] as (e: unknown) => unknown;
    const first: Record<string, string> = {};
    handler(event(first));
    const second: Record<string, string> = {};
    handler(event(second));
    // Identity, not equality: nothing was interpolated between runs.
    expect(second["zu-dispatch"]).toBe(first["zu-dispatch"]);
  });

  test("never assigns systemPrompt on a sections-only event object", () => {
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", DISPATCH_RULE)(pi);
    const handler = registered.get("before_agent_start")![0] as (e: unknown) => unknown;
    // The event deliberately has no `systemPrompt` key; if the handler
    // replaced the prompt it would have to create one.
    const minimal = event({ existing: "keep" });
    handler(minimal);
    expect("systemPrompt" in minimal).toBe(false);
    expect((minimal.systemPromptOptions as { sections: Record<string, string> }).sections).toEqual({
      existing: "keep",
      "zu-dispatch": DISPATCH_RULE,
    });
  });

  test("same-name re-registration: later write wins, logged once, no throw", () => {
    const lines: string[] = [];
    const log = (line: string) => lines.push(line);
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", "old text", log)(pi);
    promptSectionExtension("zu-dispatch", "new text", log)(pi);
    const handlers = registered.get("before_agent_start")!;
    expect(handlers).toHaveLength(2);
    const sections: Record<string, string> = {};
    (handlers[0] as (e: unknown) => unknown)(event(sections));
    (handlers[1] as (e: unknown) => unknown)(event(sections));
    expect(sections["zu-dispatch"]).toBe("new text");
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("zu-dispatch");
  });

  test("identical re-registration (reload) overwrites silently", () => {
    const lines: string[] = [];
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", DISPATCH_RULE, (l) => lines.push(l))(pi);
    promptSectionExtension("zu-dispatch", DISPATCH_RULE, (l) => lines.push(l))(pi);
    const sections: Record<string, string> = {};
    for (const handler of registered.get("before_agent_start")!) {
      (handler as (e: unknown) => unknown)(event(sections));
    }
    expect(sections["zu-dispatch"]).toBe(DISPATCH_RULE);
    expect(lines).toHaveLength(0);
  });

  test("harmless when the event carries no usable sections map", () => {
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-dispatch", DISPATCH_RULE)(pi);
    const handler = registered.get("before_agent_start")![0] as (e: unknown) => unknown;
    expect(handler(event())).toBeUndefined();
    expect(handler({})).toBeUndefined();
    expect(handler({ systemPromptOptions: {} })).toBeUndefined();
    expect(handler({ systemPromptOptions: { sections: null } })).toBeUndefined();
    expect(handler({ systemPromptOptions: { sections: "nope" } })).toBeUndefined();
  });

  test("section name is a parameter: the mechanism is generic (A4/A2 reuse)", () => {
    const { pi, registered } = fakePi();
    promptSectionExtension("zu-artifacts", "artifacts rule")(pi);
    const handler = registered.get("before_agent_start")![0] as (e: unknown) => unknown;
    const sections: Record<string, string> = {};
    handler(event(sections));
    expect(sections["zu-artifacts"]).toBe("artifacts rule");
    expect(sections["zu-dispatch"]).toBeUndefined();
  });
});
