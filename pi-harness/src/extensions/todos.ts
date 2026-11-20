/**
 * The `todos` extension entry (A2, issue #13): the plan tools, the
 * `zu-todos` prompt section, and the bundled skill, behind the B3
 * registry's single switch.
 *
 * The entry is built per session (the store is per session), so — like A1's
 * ask entry — it is composed in `PiHarnessSession.build` rather than living
 * in the static `ZU_EXTENSIONS`. Turning the extension off removes the six
 * tools, the skill (its dir rides `skillDirs`), and the prompt section
 * together; the host channel (`_zu/todos/*`) stays available, because the
 * panel edits the store the same tools read, not the extension.
 */

import type { ZuExtension } from "./registry";
import { bundledSkillDir } from "./registry";
import { todosTools } from "../todos/tools";

/**
 * The registry entry: one name (the `disabledExtensions` key), the factory,
 * and the skill root this extension owns.
 */
export function todosExtensionEntry(sessionId: string, agentDir: string): ZuExtension {
  return {
    name: "todos",
    factory: () => todosTools({ sessionId, agentDir }),
    skillDirs: [bundledSkillDir("todos")],
  };
}
