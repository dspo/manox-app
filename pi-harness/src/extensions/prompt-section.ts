/**
 * Byte-stable, named prompt sections for the captain session.
 *
 * Mechanism: pi fires `before_agent_start` before every provider request and
 * hands each handler a mutable `event.systemPromptOptions.sections` map. Pi
 * diffs the named sections one by one against the transcript's system message,
 * so writing a single section ships a one-section patch — the rest of the
 * prompt (and with it the provider's prefix cache) is left untouched.
 *
 * Two rules this factory enforces, and why:
 * 1. `text` must be a module-level `const` at the call site. It rides every
 *    request; anything dynamic (cwd, time, session state, live lists) would
 *    silently bust the provider cache turn after turn.
 * 2. The handler deliberately returns nothing. `BeforeAgentStartEventResult`
 *    allows replacing the complete prompt via `systemPrompt`; doing so drops
 *    every named section, ours included, and forces a full-prompt re-render.
 *
 * Section names are globally unique and follow `zu-<concern>`
 * (`zu-dispatch` here; later consumers reserve `zu-artifacts`, `zu-todos`).
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** Optional logger; used to report a same-name clobber, one line each. */
type SectionLog = (line: string) => void;

/**
 * The captain's delegation rule — the first consumer of the section mechanism.
 * Module-level const on purpose: it rides every captain request, and any edit
 * (or interpolation) invalidates the provider prefix cache for the section.
 */
export const DISPATCH_RULE =
  "A question one `read` can answer is not a delegation. When you do dispatch, give the subagent the full task context, not a pointer to it.";

/**
 * Register an always-on prompt section. The factory does no interpolation and
 * the handler writes only `event.systemPromptOptions.sections[section]`;
 * nothing else on the event is read or written.
 */
export function promptSectionExtension(section: string, text: string, log?: SectionLog) {
  return (pi: ExtensionAPI): void => {
    pi.on("before_agent_start", (event) => {
      const sections = event.systemPromptOptions?.sections;
      if (!sections || typeof sections !== "object") return;
      const previous = sections[section];
      sections[section] = text;
      if (previous !== undefined && previous !== text) {
        // Same-name re-registration: the later write wins. Log, don't throw —
        // a resource reload may legitimately re-run this factory.
        log?.(`prompt section "${section}" overwritten by a later registration`);
      }
      // Deliberately no return: a returned systemPrompt replaces the whole
      // prompt for the turn and discards every named section, ours included.
    });
  };
}
